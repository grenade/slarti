#![doc = r#"
slarti-ssh library API

This crate provides a small async API for:
- Checking if the remote agent is present and runnable via `ssh -T`.
- Running the agent via `ssh -T "<remote>/slarti-remote --stdio"`.
- Performing a versioned Hello/HelloAck handshake using slarti-proto.
- Sending/receiving JSON line-delimited commands and responses.

Notes:
- This library shells out to the system `ssh` binary and thus inherits
  the user's SSH config (keys, ProxyJump, etc).
- All remote commands funnel through a generic ssh runner that captures
  stdout/stderr and emits structured debug logs.

Example:

```ignore
use slarti_ssh::{check_agent, run_agent, AgentClient, AgentStatus};
use std::time::Duration;

# tokio::main
async fn main() -> anyhow::Result<()> {
    let status = check_agent("user@host", "~/.local/share/slarti/agent/0.1.0/slarti-remote", Duration::from_secs(3)).await?;
    if !status.present {
        println!("Agent is missing or cannot run");
        return Ok(());
    }

    let mut client = run_agent("user@host", "~/.local/share/slarti/agent/0.1.0/slarti-remote").await?;
    let hello = client.hello(env!("CARGO_PKG_VERSION"), Some(Duration::from_secs(3))).await?;
    println!("Connected to agent {} with capabilities {:?}", hello.agent_version, hello.capabilities);

    // Send more commands / read responses...
    Ok(())
}
```
"#]

use anyhow::{anyhow, Context as _, Result};
use slarti_proto::{Command, Response};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command as TokioCommand};
use tracing::debug;

async fn ssh_run_capture(
    target: &str,
    script: &str,
    dur: std::time::Duration,
) -> anyhow::Result<(std::process::ExitStatus, String, String)> {
    let connect_timeout = format!("ConnectTimeout={}", dur.as_secs());
    let started = std::time::Instant::now();

    let mut cmd = tokio::process::Command::new("ssh");
    cmd.envs(std::env::vars());
    cmd.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg(&connect_timeout)
        .arg("-o")
        .arg("ConnectionAttempts=1")
        .arg("-o")
        .arg("Compression=yes")
        .arg("-T")
        .arg(target)
        .arg("--")
        .arg(format!("'{}'", script))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let out = cmd.output().await.context("failed to run ssh")?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let exit_code = out.status.code();
    #[cfg(unix)]
    let exit_signal = std::os::unix::process::ExitStatusExt::signal(&out.status);
    #[cfg(not(unix))]
    let exit_signal: Option<i32> = None;

    debug!(
        target: "slarti_ssh",
        "ssh_run_capture: target={} elapsed={:?} status={} exit_code={:?} exit_signal={:?} stdout_len={} stderr_len={}",
        target,
        started.elapsed(),
        out.status,
        exit_code,
        exit_signal,
        out.stdout.len(),
        out.stderr.len(),
    );

    let stdout_trimmed = stdout.trim();
    if !stdout_trimmed.is_empty() {
        debug!(target: "slarti_ssh", "ssh_run_capture stdout: {}", stdout_trimmed);
    }
    let stderr_trimmed = stderr.trim();
    if !stderr_trimmed.is_empty() {
        debug!(target: "slarti_ssh", "ssh_run_capture stderr: {}", stderr_trimmed);
    }

    Ok((out.status, stdout, stderr))
}

/// Result of checking a remote agent via `ssh -T <target> -- <remote_path> --version`
#[derive(Debug, Clone)]
pub struct AgentStatus {
    /// Whether we could invoke the agent and get a version string
    pub present: bool,
    /// Parsed version string printed by `--version` (if present)
    pub version: Option<String>,
    /// Path used on the remote host
    pub remote_path: String,
    /// Whether the agent appears runnable (present && exit code == 0)
    pub can_run: bool,
    /// Raw stdout from the check command
    pub stdout: String,
    /// Raw stderr from the check command
    pub stderr: String,
}

/// A running agent session via `ssh -T` with JSON-over-stdio.
///
/// Use `hello` to perform the handshake, then `send_command`/`read_response` for request/response.
///
/// The session owns the ssh child process. Dropping it will terminate the session.
pub struct AgentClient {
    child: Child,
    reader: BufReader<ChildStdout>,
    writer: BufWriter<ChildStdin>,
}

impl AgentClient {
    /// Perform Hello/HelloAck handshake and return the parsed HelloAck response.
    pub async fn hello(
        &mut self,
        client_version: &str,
        read_timeout: Option<Duration>,
    ) -> Result<HelloAck> {
        let id = 1;
        let cmd = Command::Hello {
            id,
            client_version: client_version.to_string(),
        };
        self.send_command(&cmd).await?;

        let resp = if let Some(dur) = read_timeout {
            tokio::time::timeout(dur, self.read_response_line()).await??
        } else {
            self.read_response_line().await?
        };

        match resp {
            Response::HelloAck {
                id: rid,
                agent_version,
                capabilities,
            } if rid == id => Ok(HelloAck {
                agent_version,
                capabilities,
            }),
            Response::Error { id: rid, message } if rid == id => {
                Err(anyhow!("agent hello error: {}", message))
            }
            other => Err(anyhow!("unexpected response to Hello: {:?}", other)),
        }
    }

    /// Send a JSON command line to the agent (newline-delimited).
    pub async fn send_command(&mut self, cmd: &Command) -> Result<()> {
        let line = serde_json::to_string(cmd).context("serialize command to JSON")? + "\n";
        self.writer
            .write_all(line.as_bytes())
            .await
            .context("write command to agent")?;
        self.writer.flush().await.context("flush agent stdin")?;
        Ok(())
    }

    /// Read a single response (newline-delimited JSON).
    pub async fn read_response_line(&mut self) -> Result<Response> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .await
            .context("read agent stdout")?;
        if n == 0 {
            return Err(anyhow!("agent stdout closed"));
        }
        let resp: Response =
            serde_json::from_str(line.trim()).context("parse JSON response from agent")?;
        Ok(resp)
    }

    /// Attempt to gracefully terminate the ssh subprocess.
    pub async fn terminate(mut self) -> Result<()> {
        // Try to flush and shutdown stdin to signal EOF without moving fields.
        let _ = self.writer.flush().await;
        let _ = tokio::io::AsyncWriteExt::shutdown(&mut self.writer).await;

        // Attempt to wait briefly; otherwise kill the child.
        if tokio::time::timeout(Duration::from_millis(500), self.child.wait())
            .await
            .is_ok()
        {
            Ok(())
        } else {
            let _ = self.child.kill().await;
            Ok(())
        }
    }
}

impl Drop for AgentClient {
    fn drop(&mut self) {
        // Best-effort kill if still running, without blocking.
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Ok(Some(status)) = self.child.try_wait() {
                let _ = status.signal(); // just consume
            } else {
                let _ = self.child.start_kill();
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.start_kill();
        }
    }
}

/// Parsed HelloAck payload returned by the agent.
#[derive(Debug, Clone)]
pub struct HelloAck {
    pub agent_version: String,
    pub capabilities: Vec<slarti_proto::Capability>,
}

/// Check if the agent is present/runnable at the given remote path by invoking:
/// `ssh -T <target> -- <remote_path> --version`
///
/// Returns an `AgentStatus` with parsed stdout/stderr and basic flags.
pub async fn check_agent(target: &str, remote_path: &str, dur: Duration) -> Result<AgentStatus> {
    // Always pass a single-quoted remote command so the remote shell performs expansions.
    let cmd = format!("{} --version", remote_path);

    let started = std::time::Instant::now();
    let (status, stdout, stderr) = ssh_run_capture(target, &cmd, dur).await?;

    let exit_code = status.code();
    #[cfg(unix)]
    let exit_signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let exit_signal: Option<i32> = None;

    debug!(
        target: "slarti_ssh",
        "check_agent: target={} elapsed={:?} status={} exit_code={:?} exit_signal={:?} stdout_len={} stderr_len={}",
        target,
        started.elapsed(),
        status,
        exit_code,
        exit_signal,
        stdout.len(),
        stderr.len()
    );
    let stdout_trimmed = stdout.trim();
    if !stdout_trimmed.is_empty() {
        debug!(target: "slarti_ssh", "check_agent stdout: {}", stdout_trimmed);
    }
    let stderr_trimmed = stderr.trim();
    if !stderr_trimmed.is_empty() {
        debug!(target: "slarti_ssh", "check_agent stderr: {}", stderr_trimmed);
    }

    if !status.success() {
        // Normalize common "missing/not executable" cases to a non-fatal status so the UI can offer Deploy.
        let looks_missing_or_not_exec = matches!(exit_code, Some(126) | Some(127))
            || stderr.contains("No such file or directory")
            || stderr.contains("not found")
            || stderr.contains("Permission denied")
            || stderr.contains("Exec format error")
            || stderr.contains("cannot execute");

        if looks_missing_or_not_exec {
            return Ok(AgentStatus {
                present: false,
                version: None,
                remote_path: remote_path.to_string(),
                can_run: false,
                stdout,
                stderr,
            });
        }

        return Err(anyhow!(
            "ssh check failed (status={}): stderr=`{}`, stdout=`{}`",
            status,
            stderr_trimmed,
            stdout_trimmed
        ));
    }

    // Extract first non-empty trimmed line as version, if any.
    let version = stdout
        .lines()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(|s| s.to_string());

    Ok(AgentStatus {
        present: version.is_some(),
        version,
        remote_path: remote_path.to_string(),
        can_run: true,
        stdout,
        stderr,
    })
}

/// Run the agent via `ssh -T <target> -- <remote_path> --stdio`, returning an `AgentClient`
/// ready to perform the Hello handshake and subsequent commands.
///
/// This does not perform the handshake automatically so the caller can decide how to handle
/// version/capability mismatches.
pub async fn run_agent(target: &str, remote_path: &str) -> Result<AgentClient> {
    let mut cmd = TokioCommand::new("ssh");
    let started = std::time::Instant::now();
    cmd.envs(std::env::vars());
    debug!(target: "slarti_ssh", "run_agent: target={} remote_path={}", target, remote_path);
    cmd.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .arg("-o")
        .arg("ConnectionAttempts=1")
        .arg("-o")
        .arg("Compression=yes")
        .arg("-T")
        .arg(target)
        .arg("--")
        // Pass a single-quoted remote command so the remote shell expands $HOME and ~.
        .arg(format!("'{} --stdio'", remote_path));

    debug!(target: "slarti_ssh", "run_agent: spawning (started {:?})", started);

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = cmd.spawn().context("spawn ssh -T for agent")?;

    let stdin: ChildStdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("agent stdin not available"))?;
    let stdout: ChildStdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("agent stdout not available"))?;

    let reader = BufReader::new(stdout);
    let writer = BufWriter::new(stdin);

    Ok(AgentClient {
        child,
        reader,
        writer,
    })
}

/// Determine if the remote user is root by querying `id -u` over SSH.
/// Returns true if the UID is 0.
pub async fn remote_user_is_root(target: &str, dur: Duration) -> Result<bool> {
    let (_status, stdout, _stderr) = ssh_run_capture(target, "id -u", dur).await?;
    Ok(stdout.trim() == "0")
}

/// Result of agent deployment.
#[derive(Debug, Clone)]
pub struct DeployResult {
    pub remote_path: String,
    pub used_rsync: bool,
}

/// Deploy the agent to a hard-coded path on the remote host:
/// - Non-root: $HOME/.local/share/slarti/agent/<version>/slarti-remote
/// - Root:     /usr/local/lib/slarti/agent/<version>/slarti-remote
///
/// The `local_artifact` can be a binary or a .tar.gz archive containing
/// `bin/slarti-remote`. rsync is preferred; scp is used as a fallback.
pub async fn deploy_agent(
    target: &str,
    local_artifact: &Path,
    version: &str,
    timeout: Duration,
) -> Result<DeployResult> {
    // Decide install dir based on remote user.
    let is_root = remote_user_is_root(target, timeout).await.unwrap_or(false);
    let (remote_dir_abs, remote_dir_rsync_dst, remote_path_for_agent) = if is_root {
        let dir = format!("/usr/local/lib/slarti/agent/{}", version);
        (dir.clone(), dir.clone(), format!("{}/slarti-remote", dir))
    } else {
        // For rsync, use relative-to-home path; for mkdir/mv/chmod use $HOME via the shell.
        let rel = format!(".local/share/slarti/agent/{}", version);
        (
            format!("$HOME/{}", rel),
            rel.clone(),
            format!("$HOME/{}/slarti-remote", rel.clone()),
        )
    };

    debug!(
        target: "slarti_ssh",
        "deploy: target={} version={} remote_dir_abs={} rsync_dst={} artifact={:?}",
        target, version, remote_dir_abs, remote_dir_rsync_dst, local_artifact
    );

    // Ensure target directory exists (shell expansion handles $HOME for non-root)
    let mkdir_script = format!("'mkdir -p {remote_dir_abs}'");
    let (st_mkdir, _so_mkdir, _se_mkdir) = ssh_run_capture(target, &mkdir_script, timeout).await?;
    if !st_mkdir.success() {
        return Err(anyhow!("remote mkdir failed on {}", target));
    }

    // Upload via rsync to directory (relative for non-root, absolute for root)
    let rsync_dst = format!("{}:{}", target, remote_dir_rsync_dst);
    debug!(target: "slarti_ssh", "deploy: rsync {:?} -> {}", local_artifact, rsync_dst);
    let rsync_status = TokioCommand::new("rsync")
        .arg("-az")
        .arg("--chmod=755")
        .arg(local_artifact.as_os_str())
        .arg(&rsync_dst)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    let mut uploaded = false;
    let mut used_rsync = false;
    if let Ok(status) = rsync_status.await {
        debug!(target: "slarti_ssh", "deploy: rsync status={}", status);
        if status.success() {
            used_rsync = true;
            uploaded = true;
        }
    }

    // Fallback to scp if needed
    let file_name = PathBuf::from(local_artifact)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "slarti-remote".to_string());
    if !uploaded {
        debug!(target: "slarti_ssh", "deploy: rsync failed, falling back to scp");
        let scp_dst = format!("{}:{}/{}", target, remote_dir_rsync_dst, file_name);
        let scp_status = TokioCommand::new("scp")
            .arg(local_artifact.as_os_str())
            .arg(&scp_dst)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context("scp failed to run")?;
        debug!(target: "slarti_ssh", "deploy: scp status={}", scp_status);
        if scp_status.success() {
            uploaded = true;
        }
    }

    if !uploaded {
        return Err(anyhow!("failed to upload agent (rsync/scp) to {}", target));
    }

    // If uploaded basename differs, move and chmod in a single remote script.
    if file_name != "slarti-remote" {
        let mv_script = format!(
            "mv -- {dir}/{name} {dir}/slarti-remote && chmod 755 -- {dir}/slarti-remote",
            dir = remote_dir_abs,
            name = file_name
        );
        debug!(target: "slarti_ssh", "deploy: {}", mv_script);
        let (st_mv, _so_mv, _se_mv) = ssh_run_capture(target, &mv_script, timeout).await?;
        if !st_mv.success() {
            return Err(anyhow!("remote move/chmod failed on {}", target));
        }
    } else if !used_rsync {
        // Ensure perms if we used scp
        let chmod_script = format!("chmod 755 -- {}", remote_path_for_agent);
        debug!(target: "slarti_ssh", "deploy: {}", chmod_script);
        let (st_chmod, _so_chmod, _se_chmod) =
            ssh_run_capture(target, &chmod_script, timeout).await?;
        if !st_chmod.success() {
            return Err(anyhow!("remote chmod failed on {}", target));
        }
    }

    Ok(DeployResult {
        remote_path: remote_path_for_agent,
        used_rsync,
    })
}

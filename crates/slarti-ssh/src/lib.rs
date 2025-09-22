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
- For deployment (rsync/scp), a follow-up will extend this crate with
  file sync helpers.

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
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command as TokioCommand};
use tracing::debug;

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
    let needs_shell = remote_path.contains('~') || remote_path.contains('$');

    let connect_timeout = format!("ConnectTimeout={}", dur.as_secs());
    let mut cmd = TokioCommand::new("ssh");
    // Start timing and prepare a debuggable command string
    let started = std::time::Instant::now();
    let dbg_cmd: String;
    cmd.envs(std::env::vars()); // inherit environment to respect user SSH config (SSH_AUTH_SOCK, etc.)
    debug!(target: "slarti_ssh", "check_agent: target={} dur={:?} remote_path={} needs_shell={}", target, dur, remote_path, needs_shell);
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
        .arg("--");
    if needs_shell {
        // Use a shell so $HOME / ~ expansion works on remote
        cmd.arg("sh")
            .arg("-c")
            .arg(format!("{} --version", remote_path));
    } else {
        cmd.arg(remote_path).arg("--version");
    }
    // Build a debuggable command string for diagnostics (does not affect execution)
    dbg_cmd = {
        let mut s = format!(
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o {} -o Compression=yes -T {} -- ",
            &connect_timeout,
            target
        );
        if needs_shell {
            s.push_str(&format!("sh -c '{} --version'", remote_path));
        } else {
            s.push_str(&format!("{} --version", remote_path));
        }
        s
    };

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let out = cmd
        .output()
        .await
        .with_context(|| format!("failed to run ssh (cmd={})", dbg_cmd))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let exit_code = out.status.code();
    #[cfg(unix)]
    let exit_signal = std::os::unix::process::ExitStatusExt::signal(&out.status);
    #[cfg(not(unix))]
    let exit_signal: Option<i32> = None;

    debug!(
        target: "slarti_ssh",
        "check_agent: target={} elapsed={:?} status={} exit_code={:?} exit_signal={:?} stdout_len={} stderr_len={} cmd={}",
        target,
        started.elapsed(),
        out.status,
        exit_code,
        exit_signal,
        out.stdout.len(),
        out.stderr.len(),
        dbg_cmd
    );

    if !out.status.success() {
        // Normalize common "missing/not executable" cases to a non-fatal status so the UI can offer Deploy.
        let code = out.status.code();
        let looks_missing_or_not_exec = matches!(code, Some(126) | Some(127)) || // 126: found but not executable, 127: not found
            stderr.contains("No such file or directory") ||
            stderr.contains("not found") ||
            stderr.contains("Permission denied") ||
            stderr.contains("Exec format error") ||     // ENOEXEC
            stderr.contains("cannot execute"); // common shell error message

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

        // For other failures (e.g., ssh handshake/proxy issues), surface as an error.
        return Err(anyhow!(
            "ssh check failed (status={}): stderr=`{}`, stdout=`{}`",
            out.status,
            stderr.trim(),
            stdout.trim()
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
    let needs_shell = remote_path.contains('~') || remote_path.contains('$');

    let mut cmd = TokioCommand::new("ssh");
    let started = std::time::Instant::now();
    let dbg_cmd: String;
    cmd.envs(std::env::vars());
    debug!(target: "slarti_ssh", "run_agent: target={} remote_path={} needs_shell={}", target, remote_path, needs_shell);
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
        .arg("--");
    if needs_shell {
        // Use a shell so $HOME / ~ expansion works on remote
        cmd.arg("sh")
            .arg("-c")
            .arg(format!("{} --stdio", remote_path));
    } else {
        cmd.arg(remote_path).arg("--stdio");
    }
    // Build a debuggable command string for diagnostics (does not affect execution)
    dbg_cmd = {
        let mut s = format!(
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=5 -o Compression=yes -T {} -- ",
            target
        );
        if needs_shell {
            s.push_str(&format!("sh -c '{} --stdio'", remote_path));
        } else {
            s.push_str(&format!("{} --stdio", remote_path));
        }
        s
    };
    debug!(
        target: "slarti_ssh",
        "run_agent: spawning: cmd={} (started {:?})",
        dbg_cmd, started
    );

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit()); // inherit stderr to expose remote errors interactively

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
    let connect_timeout = format!("ConnectTimeout={}", dur.as_secs());
    let mut cmd = TokioCommand::new("ssh");
    cmd.envs(std::env::vars());
    debug!(target: "slarti_ssh", "remote_user_is_root: target={} dur={:?}", target, dur);
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
        .arg("sh")
        .arg("-lc")
        .arg("id -u")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let out = cmd.output().await.context("failed to run ssh for id -u")?;

    let exit_code = out.status.code();
    #[cfg(unix)]
    let exit_signal = std::os::unix::process::ExitStatusExt::signal(&out.status);
    #[cfg(not(unix))]
    let exit_signal: Option<i32> = None;
    debug!(
        target: "slarti_ssh",
        "remote_user_is_root: status={} exit_code={:?} exit_signal={:?}",
        out.status,
        exit_code,
        exit_signal
    );

    if !out.status.success() {
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(stdout == "0")
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
    // Decide installation paths based on remote user.
    let is_root = remote_user_is_root(target, timeout).await.unwrap_or(false);
    let remote_dir = if is_root {
        format!("/usr/local/lib/slarti/agent/{}", version)
    } else {
        format!("$HOME/.local/share/slarti/agent/{}", version)
    };
    let remote_path = format!("{}/slarti-remote", remote_dir);

    // Ensure target directory exists on remote
    let connect_timeout = format!("ConnectTimeout={}", timeout.as_secs());
    let mut ssh_mkdir = TokioCommand::new("ssh");
    ssh_mkdir.envs(std::env::vars());
    // Optional verbose debugging when SLARTI_SSH_DEBUG is set
    if std::env::var("SLARTI_SSH_DEBUG").is_ok() {
        ssh_mkdir.arg("-vvv");
    }
    ssh_mkdir
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg(&connect_timeout)
        .arg("-o")
        .arg("Compression=yes")
        .arg("-T")
        .arg(target)
        .arg("--")
        .arg("sh")
        .arg("-lc")
        .arg(format!("mkdir -p \"{dir}\"", dir = remote_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = tokio::time::timeout(timeout, ssh_mkdir.status())
        .await
        .map_err(|_| anyhow!("ssh mkdir timed out after {:?}", timeout))??;

    if !status.success() {
        return Err(anyhow!("remote mkdir failed on {}", target));
    }

    // Upload artifact via rsync first directly to the final path, fallback to scp if rsync fails.
    let mut used_rsync = false;
    let rsync_dst = format!("{}:{}", target, remote_path);

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
    if let Ok(Ok(status)) = tokio::time::timeout(timeout, rsync_status).await {
        if status.success() {
            used_rsync = true;
            uploaded = true;
        }
    }

    if !uploaded {
        let scp_status = TokioCommand::new("scp")
            .arg(local_artifact.as_os_str())
            .arg(&rsync_dst)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        if let Ok(Ok(status)) = tokio::time::timeout(timeout, scp_status).await {
            if status.success() {
                uploaded = true;
            }
        }
    }

    if !uploaded {
        return Err(anyhow!("failed to upload agent (rsync/scp) to {}", target));
    }

    // Ensure executable permissions on the remote binary only if scp fallback was used
    if !used_rsync {
        let mut ssh_chmod = TokioCommand::new("ssh");
        ssh_chmod.envs(std::env::vars());
        if std::env::var("SLARTI_SSH_DEBUG").is_ok() {
            ssh_chmod.arg("-vvv");
        }
        ssh_chmod
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg("-o")
            .arg(&connect_timeout)
            .arg("-o")
            .arg("Compression=yes")
            .arg("-T")
            .arg(target)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(format!("chmod 755 \"{path}\"", path = remote_path))
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let status = tokio::time::timeout(timeout, ssh_chmod.status())
            .await
            .map_err(|_| anyhow!("ssh chmod timed out after {:?}", timeout))??;

        if !status.success() {
            return Err(anyhow!("remote chmod failed on {}", target));
        }
    }

    Ok(DeployResult {
        remote_path,
        used_rsync,
    })
}

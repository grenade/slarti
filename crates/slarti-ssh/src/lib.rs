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
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command as TokioCommand};

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
    let mut cmd = TokioCommand::new("ssh");
    cmd.arg("-T")
        .arg(target)
        .arg("--")
        .arg(remote_path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let out = tokio::time::timeout(dur, cmd.output())
        .await
        .map_err(|_| anyhow!("ssh check timed out after {:?}", dur))?
        .context("failed to run ssh")?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    // Extract first non-empty trimmed line as version, if any.
    let version = stdout
        .lines()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(|s| s.to_string());

    Ok(AgentStatus {
        present: out.status.success() && version.is_some(),
        version,
        remote_path: remote_path.to_string(),
        can_run: out.status.success(),
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
    let mut child = TokioCommand::new("ssh")
        .arg("-T")
        .arg(target)
        .arg("--")
        .arg(remote_path)
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit()) // inherit stderr to expose remote errors interactively
        .spawn()
        .context("spawn ssh -T for agent")?;

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

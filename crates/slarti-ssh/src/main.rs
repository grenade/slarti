use anyhow::{anyhow, Result};
use clap::Parser;
use slarti_proto::Command as CommandMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

#[derive(Parser, Debug)]
struct Args {
    /// SSH target, e.g. user@host
    target: String,
    /// Remote agent command
    #[arg(long, default_value = "./slarti-remote")]
    agent: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let mut child = Command::new("ssh")
        .arg("-T")
        .arg(&args.target)
        .arg("--")
        .arg(&args.agent)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()?;

    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;

    let req = CommandMsg::ListDir {
        id: 1,
        path: "~/".into(),
        max: Some(200),
        skip: Some(0),
    };
    let line = serde_json::to_string(&req)? + "\n";
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await?;

    // Read one line of JSON response
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        let n = stdout.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&buf[..pos]).into_owned();
            println!("RESPONSE: {}", line);
            break;
        }
    }
    Ok(())
}

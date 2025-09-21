use anyhow::Result;
use clap::Parser;
use slarti_proto::{Command, Response};
use slarti_ssh::{check_agent, run_agent};
use std::time::Duration;

#[derive(Parser, Debug)]
struct Args {
    /// SSH target, e.g. user@host
    target: String,
    /// Remote agent path on target host
    #[arg(long, default_value = "./slarti-remote")]
    agent: String,
    /// Timeout (seconds) for checks and handshake
    #[arg(long, default_value_t = 3u64)]
    timeout: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let timeout = Duration::from_secs(args.timeout);

    // 1) Check agent presence/version
    let status = check_agent(&args.target, &args.agent, timeout).await?;
    println!(
        "check_agent: present={}, can_run={}, version={:?}",
        status.present, status.can_run, status.version
    );

    // 2) Run agent and perform Hello/HelloAck handshake
    let mut client = run_agent(&args.target, &args.agent).await?;
    let hello = client
        .hello(env!("CARGO_PKG_VERSION"), Some(timeout))
        .await?;
    println!(
        "HELLO: agent_version={}, capabilities={:?}",
        hello.agent_version, hello.capabilities
    );

    // 3) Sample command after handshake: ListDir ~/ (first 200 entries)
    let req = Command::ListDir {
        id: 2,
        path: "~/".into(),
        max: Some(200),
        skip: Some(0),
    };
    client.send_command(&req).await?;
    match client.read_response_line().await? {
        Response::ListDirOk { id, entries, eof } => {
            println!("ListDirOk id={} entries={} eof={}", id, entries.len(), eof);
            for e in entries.iter().take(10) {
                println!(
                    "{}\t{}\t{}",
                    if e.is_dir { "DIR " } else { "FILE" },
                    e.name,
                    e.path
                );
            }
            if entries.len() > 10 {
                println!("... (showing first 10)");
            }
        }
        other => {
            println!("Unexpected response: {:?}", other);
        }
    }

    Ok(())
}

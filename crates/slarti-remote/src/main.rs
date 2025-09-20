use anyhow::{anyhow, Result};
use slarti_proto::{Command, DirEntry, Response};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() -> Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = tokio::io::BufWriter::new(stdout);

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Command>(&line) {
            Ok(cmd) => handle_command(cmd).await,
            Err(e) => Err(anyhow!("invalid json: {}", e)),
        };

        let json_line = match resp {
            Ok(r) => serde_json::to_string(&r)?,
            Err(e) => serde_json::to_string(&Response::Error {
                id: 0,
                message: e.to_string(),
            })?,
        };
        writer.write_all(json_line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

async fn handle_command(cmd: Command) -> Result<Response> {
    match cmd {
        Command::ListDir {
            id,
            path,
            max,
            skip,
        } => {
            let max = max.unwrap_or(2000).min(10_000);
            let skip = skip.unwrap_or(0);
            let dir = PathBuf::from(expand_tilde(path));

            let mut entries = Vec::new();
            let mut read_dir = fs::read_dir(&dir)
                .await
                .map_err(|e| anyhow!("read_dir({:?}): {}", dir, e))?;

            while let Some(ent) = read_dir.next_entry().await? {
                let meta = ent.metadata().await?;
                entries.push(DirEntry {
                    name: ent.file_name().to_string_lossy().to_string(),
                    path: ent.path().to_string_lossy().to_string(),
                    is_dir: meta.is_dir(),
                    size: if meta.is_file() {
                        Some(meta.len())
                    } else {
                        None
                    },
                });
            }

            entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });

            let eof = skip + max >= entries.len();
            let slice = entries.into_iter().skip(skip).take(max).collect::<Vec<_>>();
            Ok(Response::ListDirOk {
                id,
                entries: slice,
                eof,
            })
        }
    }
}

fn expand_tilde(path: String) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs_next::home_dir() {
            return format!("{}/{}", home.display(), stripped);
        }
    }
    path
}

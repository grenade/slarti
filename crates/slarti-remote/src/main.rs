use anyhow::{anyhow, Result};
use slarti_proto::{Capability, Command, DirEntry, Response, ServiceInfo, StaticConfig, SysInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;

const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    // Print agent version and exit if requested.
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("{}", AGENT_VERSION);
        return Ok(());
    }
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
        Command::Hello {
            id,
            client_version: _,
        } => Ok(Response::HelloAck {
            id,
            agent_version: AGENT_VERSION.to_string(),
            capabilities: vec![
                Capability::SysInfo,
                Capability::StaticConfig,
                Capability::ServicesList,
                Capability::ContainersList,
                Capability::NetListeners,
                Capability::ProcessesSummary,
            ],
        }),
        Command::SysInfo { id } => {
            let info = sys_info().await?;
            Ok(Response::SysInfoOk { id, info })
        }
        Command::StaticConfig { id } => {
            let config = static_config().await?;
            Ok(Response::StaticConfigOk { id, config })
        }
        Command::ServicesList { id } => {
            let services = services_list().await?;
            Ok(Response::ServicesListOk { id, services })
        }
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

async fn sys_info() -> Result<SysInfo> {
    // OS and arch from Rust std
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();

    // Kernel release from /proc (Linux)
    let kernel = match fs::read_to_string("/proc/sys/kernel/osrelease").await {
        Ok(s) => s.trim().to_string(),
        Err(_) => "unknown".to_string(),
    };

    // Uptime in seconds (first field of /proc/uptime)
    let uptime_secs = match fs::read_to_string("/proc/uptime").await {
        Ok(s) => s
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .map(|f| f as u64)
            .unwrap_or(0),
        Err(_) => 0,
    };

    // Hostname from /proc, fallback to HOSTNAME env
    let hostname = match fs::read_to_string("/proc/sys/kernel/hostname").await {
        Ok(s) => s.trim().to_string(),
        Err(_) => std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string()),
    };

    Ok(SysInfo {
        os,
        kernel,
        arch,
        uptime_secs,
        hostname,
    })
}

async fn static_config() -> Result<StaticConfig> {
    // /etc/os-release content (optional)
    let os_release = match fs::read_to_string("/etc/os-release").await {
        Ok(s) => Some(s),
        Err(_) => None,
    };

    // CPU count from /proc/cpuinfo
    let cpu_count = match fs::read_to_string("/proc/cpuinfo").await {
        Ok(s) => s.lines().filter(|l| l.starts_with("processor")).count() as u32,
        Err(_) => 0,
    };

    // MemTotal from /proc/meminfo (in kB) -> bytes
    let mem_total_bytes = match fs::read_to_string("/proc/meminfo").await {
        Ok(s) => s
            .lines()
            .find(|l| l.starts_with("MemTotal:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb * 1024)
            .unwrap_or(0),
        Err(_) => 0,
    };

    Ok(StaticConfig {
        os_release,
        cpu_count,
        mem_total_bytes,
    })
}

async fn services_list() -> Result<Vec<ServiceInfo>> {
    // Build enabled/disabled map from unit files
    let mut enabled_map: HashMap<String, Option<bool>> = HashMap::new();
    if let Ok(out) = TokioCommand::new("systemctl")
        .arg("list-unit-files")
        .arg("--type=service")
        .arg("--no-legend")
        .arg("--no-pager")
        .output()
        .await
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let mut parts = line.split_whitespace();
                if let (Some(name), Some(state)) = (parts.next(), parts.next()) {
                    let enabled = match state {
                        "enabled" | "enabled-runtime" => Some(true),
                        "disabled" => Some(false),
                        _ => None,
                    };
                    enabled_map.insert(name.to_string(), enabled);
                }
            }
        }
    }

    let mut services = Vec::new();
    if let Ok(out) = TokioCommand::new("systemctl")
        .arg("list-units")
        .arg("--type=service")
        .arg("--no-legend")
        .arg("--no-pager")
        .output()
        .await
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                // Expected columns: UNIT LOAD ACTIVE SUB DESCRIPTION
                let mut it = line.split_whitespace();
                let unit = match it.next() {
                    Some(u) => u,
                    None => continue,
                };
                // Skip LOAD
                let _load = it.next();
                let active = it.next().unwrap_or("unknown").to_string();
                let sub = it.next().unwrap_or("unknown").to_string();
                let rest: Vec<&str> = it.collect();
                let description = if rest.is_empty() {
                    None
                } else {
                    Some(rest.join(" "))
                };
                let enabled = enabled_map.get(unit).cloned().unwrap_or(None);
                services.push(ServiceInfo {
                    name: unit.to_string(),
                    description,
                    active_state: active,
                    sub_state: sub,
                    enabled,
                });
            }
        }
    }

    Ok(services)
}

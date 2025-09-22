use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Client-initiated handshake
    Hello { id: u64, client_version: String },
    /// Fetch basic system information
    SysInfo { id: u64 },
    /// Fetch static system configuration
    StaticConfig { id: u64 },
    ListDir {
        id: u64,
        path: String,
        max: Option<usize>,
        skip: Option<usize>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Agent acknowledges handshake and advertises capabilities
    HelloAck {
        id: u64,
        agent_version: String,
        capabilities: Vec<Capability>,
    },
    /// Basic system information
    SysInfoOk {
        id: u64,
        info: SysInfo,
    },
    /// Static system configuration
    StaticConfigOk {
        id: u64,
        config: StaticConfig,
    },
    ListDirOk {
        id: u64,
        entries: Vec<DirEntry>,
        eof: bool,
    },
    Error {
        id: u64,
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SysInfo {
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub uptime_secs: u64,
    pub hostname: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StaticConfig {
    pub os_release: Option<String>,
    pub cpu_count: u32,
    pub mem_total_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    SysInfo,
    StaticConfig,
    ServicesList,
    ContainersList,
    NetListeners,
    ProcessesSummary,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Client-initiated handshake
    Hello { id: u64, client_version: String },
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
#[serde(rename_all = "snake_case")]
pub enum Capability {
    SysInfo,
    StaticConfig,
    ServicesList,
    ContainersList,
    NetListeners,
    ProcessesSummary,
}

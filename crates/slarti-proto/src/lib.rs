use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
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

use serde::{Deserialize, Serialize};

/// Bundle of files which can be loaded by the runtime.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Bundle {
    /// The first lua file which will be executed on creation.
    pub entry_point: String,
    pub files: Vec<File>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct File {
    pub file_name: String,
    /// fs path of the file.
    pub path: String,
    pub content: Vec<u8>,
}

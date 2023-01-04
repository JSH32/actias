use lz4_flex::{compress_prepend_size, decompress_size_prepended};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
    #[serde(serialize_with = "as_compressed", deserialize_with = "from_compressed")]
    pub content: Vec<u8>,
}

fn as_compressed<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
    let compressed = compress_prepend_size(v);

    // Encode as base64.
    let base64 = base64::encode(compressed);
    String::serialize(&base64, s)
}

fn from_compressed<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    // Base64 decode.
    let base64 = String::deserialize(d)?;
    let bytes = base64::decode(base64.as_bytes()).map_err(|e| serde::de::Error::custom(e))?;

    Ok(decompress_size_prepended(&bytes).unwrap())
}

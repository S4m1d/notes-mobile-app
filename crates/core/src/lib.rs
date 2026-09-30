//! Shared logic of Scarlet Notes: vault file operations, markdown line
//! classification, device identity and the end-to-end encrypted sync protocol.
//! Used by both the mobile app and the desktop CLI.

pub mod channel;
pub mod markdown;
pub mod proto;
pub mod store;
pub mod sync;
pub mod vault;

pub const DEFAULT_PORT: u16 = 47800;

pub type Result<T> = std::result::Result<T, Error>;

/// A plain string error: everything here ends up as a message for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<snow::Error> for Error {
    fn from(e: snow::Error) -> Self {
        Error(format!("encryption error: {e}"))
    }
}

impl From<postcard::Error> for Error {
    fn from(e: postcard::Error) -> Self {
        Error(format!("malformed message: {e}"))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(format!("malformed file: {e}"))
    }
}

pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error(msg.into()))
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return err("invalid hex string");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| Error("invalid hex string".into())))
        .collect()
}

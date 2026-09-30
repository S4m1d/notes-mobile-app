//! Messages exchanged over the encrypted channel.
//!
//! After the handshake both sides send an [`Intro`]. If either side doesn't
//! know the other's key yet, both ask their user to confirm the pairing code
//! and exchange a [`PairAnswer`]. From then on the initiator drives the
//! session with [`Request`]s and the responder answers each with a [`Response`].

use serde::{Deserialize, Serialize};

use crate::store::BaseState;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Intro {
    pub version: u32,
    pub device_name: String,
    /// Whether the sender already has the receiver's key in its paired peers.
    pub knows_peer: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PairAnswer(pub bool);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub is_dir: bool,
    pub hash: [u8; 32],
    /// Modification time, seconds since the Unix epoch.
    pub mtime: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Start syncing the named vault.
    Sync { vault: String },
    Manifest,
    Get(String),
    Put { path: String, data: Vec<u8>, mtime: u64 },
    Delete(String),
    Mkdir(String),
    Rmdir(String),
    /// The sync is complete; `base` is the state both sides now agree on.
    Done { base: BaseState },
    /// End the session without syncing (pairing only).
    Bye,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Error(String),
    Manifest(Vec<Entry>),
    File { data: Vec<u8>, mtime: u64 },
}

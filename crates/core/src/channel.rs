//! End-to-end encrypted channel between two devices: a Noise XX handshake
//! with static device keys over a plain TCP stream, then length-framed
//! encrypted messages.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde::{de::DeserializeOwned, Serialize};
use snow::{Builder, TransportState};

use crate::{err, to_hex, Result};

const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const PROLOGUE: &[u8] = b"scarlet-notes/1";
const MAX_FRAME: usize = 65535;
const TAG_LEN: usize = 16;
const MAX_CHUNK: usize = MAX_FRAME - TAG_LEN;
/// Upper bound for one protocol message, so a peer can't make us allocate without limit.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Generates a static X25519 key pair: (private, public).
pub fn generate_keypair() -> Result<(Vec<u8>, Vec<u8>)> {
    let keypair = Builder::new(NOISE_PARAMS.parse()?).generate_keypair()?;
    Ok((keypair.private, keypair.public))
}

pub struct Channel {
    stream: TcpStream,
    noise: TransportState,
    remote_key: Vec<u8>,
    pairing_code: String,
}

fn write_frame(stream: &mut TcpStream, data: &[u8]) -> Result<()> {
    stream.write_all(&(data.len() as u16).to_be_bytes())?;
    stream.write_all(data)?;
    Ok(())
}

fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 2];
    stream.read_exact(&mut len)?;
    let mut data = vec![0u8; u16::from_be_bytes(len) as usize];
    stream.read_exact(&mut data)?;
    Ok(data)
}

impl Channel {
    /// Runs the handshake. Both sides learn each other's static public key;
    /// whether that key is trusted is decided by the caller.
    pub fn handshake(mut stream: TcpStream, private_key: &[u8], initiator: bool) -> Result<Channel> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;

        let builder = Builder::new(NOISE_PARAMS.parse()?).local_private_key(private_key)?.prologue(PROLOGUE)?;
        let mut noise = if initiator { builder.build_initiator()? } else { builder.build_responder()? };
        let mut buf = vec![0u8; MAX_FRAME];
        while !noise.is_handshake_finished() {
            if noise.is_my_turn() {
                let n = noise.write_message(&[], &mut buf)?;
                write_frame(&mut stream, &buf[..n])?;
            } else {
                let frame = read_frame(&mut stream)?;
                noise.read_message(&frame, &mut buf)?;
            }
        }
        stream.flush()?;

        // Both sides derive the same code from the handshake transcript; a
        // man-in-the-middle would make the two devices show different codes.
        let hash = to_hex(noise.get_handshake_hash());
        let pairing_code = format!("{}-{}-{}", &hash[0..4], &hash[4..8], &hash[8..12]);
        let Some(remote_key) = noise.get_remote_static().map(<[u8]>::to_vec) else {
            return err("peer did not present a key");
        };
        Ok(Channel { stream, noise: noise.into_transport_mode()?, remote_key, pairing_code })
    }

    pub fn remote_key(&self) -> &[u8] {
        &self.remote_key
    }

    pub fn pairing_code(&self) -> &str {
        &self.pairing_code
    }

    pub fn set_timeout(&mut self, timeout: Duration) -> Result<()> {
        self.stream.set_read_timeout(Some(timeout))?;
        Ok(())
    }

    pub fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        let payload = postcard::to_stdvec(msg)?;
        if payload.len() > MAX_MESSAGE {
            return err("message too large (notes are limited to 64 MiB)");
        }
        let mut buf = vec![0u8; MAX_FRAME];
        let n = self.noise.write_message(&(payload.len() as u32).to_be_bytes(), &mut buf)?;
        write_frame(&mut self.stream, &buf[..n])?;
        for chunk in payload.chunks(MAX_CHUNK) {
            let n = self.noise.write_message(chunk, &mut buf)?;
            write_frame(&mut self.stream, &buf[..n])?;
        }
        self.stream.flush()?;
        Ok(())
    }

    pub fn recv<T: DeserializeOwned>(&mut self) -> Result<T> {
        let mut buf = vec![0u8; MAX_FRAME];
        let frame = read_frame(&mut self.stream)?;
        let n = self.noise.read_message(&frame, &mut buf)?;
        if n != 4 {
            return err("malformed message header");
        }
        let total = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if total > MAX_MESSAGE {
            return err("peer sent an oversized message");
        }
        let mut payload = Vec::with_capacity(total);
        while payload.len() < total {
            let frame = read_frame(&mut self.stream)?;
            let n = self.noise.read_message(&frame, &mut buf)?;
            if n == 0 || payload.len() + n > total {
                return err("malformed message body");
            }
            payload.extend_from_slice(&buf[..n]);
        }
        Ok(postcard::from_bytes(&payload)?)
    }
}

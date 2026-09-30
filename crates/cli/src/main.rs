//! `scarlet` — desktop companion of the Scarlet Notes app. Registers local
//! note directories as vaults and syncs them with the phone over an
//! end-to-end encrypted connection.

use std::io::{BufRead, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use scarlet_core::store::{fingerprint, Store};
use scarlet_core::sync::{connect, run_initiator, run_responder, SyncUi};
use scarlet_core::{Error, Result, DEFAULT_PORT};

const USAGE: &str = "\
scarlet — sync Scarlet Notes vaults with your phone

USAGE:
    scarlet id [NAME]              show this device's identity (and optionally rename it)
    scarlet vault add NAME PATH    register a notes directory as a vault
    scarlet vault list             list registered vaults
    scarlet vault rm NAME          unregister a vault (files are kept)
    scarlet sync VAULT [ADDR]      sync a vault with the phone listening at ADDR
    scarlet serve [PORT]           wait for the phone to start a sync
    scarlet pair ADDR              pair with the phone without syncing
    scarlet peers                  list paired devices
    scarlet unpair NAME            forget a paired device

ADDR is the phone's IP address with an optional port (default 47800); when
omitted, the last used address is taken. Keys and settings are stored in
~/.config/scarlet-notes (override with SCARLET_HOME).";

struct Terminal;

impl SyncUi for Terminal {
    fn log(&mut self, line: &str) {
        println!("  {line}");
    }

    fn confirm_pairing(&mut self, peer_name: &str, code: &str) -> bool {
        println!("\nPairing request from '{peer_name}'.");
        println!("Pairing code:  {code}");
        print!("Does the other device show the same code? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        matches!(answer.trim(), "y" | "Y" | "yes")
    }
}

fn home_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("SCARLET_HOME") {
        return Ok(dir.into());
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(dir).join("scarlet-notes"));
    }
    match std::env::var_os("HOME") {
        Some(home) => Ok(PathBuf::from(home).join(".config").join("scarlet-notes")),
        None => Err(Error("can't locate the config directory: HOME is not set".into())),
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "desktop".to_string())
}

fn resolve_addr(store: &Store, addr: Option<&String>) -> Result<String> {
    match addr {
        Some(addr) => Ok(addr.clone()),
        None => match store.last_addr() {
            addr if addr.is_empty() => Err(Error("no address given and none used before".into())),
            addr => Ok(addr),
        },
    }
}

fn serve(store: &Store, port: u16) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    println!("Listening on port {port}. Start the sync on your phone; Ctrl-C to stop.");
    let vaults = store.vaults()?;
    let resolve_vault = |name: &str| -> Result<PathBuf> {
        vaults.get(name).cloned().ok_or(Error(format!(
            "vault '{name}' is not registered on the desktop (scarlet vault add {name} PATH)"
        )))
    };
    for stream in listener.incoming() {
        let stream = stream?;
        println!("Connection from {}", stream.peer_addr().map(|a| a.to_string()).unwrap_or_default());
        match run_responder(store, stream, &resolve_vault, &mut Terminal) {
            Ok(summary) => println!("{}", summary.describe()),
            Err(e) => eprintln!("Session failed: {e}"),
        }
    }
    Ok(())
}

fn run(args: &[String]) -> Result<()> {
    let store = Store::open(home_dir()?)?;
    let identity = store.identity_or_create(&hostname())?;
    let arg = |i: usize| args.get(i).map(String::as_str);

    match (arg(0), arg(1)) {
        (Some("id"), name) => {
            if let Some(name) = name {
                store.set_device_name(name)?;
            }
            let identity = store.identity()?;
            println!("Device name:  {}", identity.device_name);
            println!("Key:          {}", identity.fingerprint());
        }
        (Some("vault"), Some("add")) => {
            let (Some(name), Some(path)) = (arg(2), arg(3)) else {
                return Err(Error("usage: scarlet vault add NAME PATH".into()));
            };
            store.add_vault(name, Path::new(path))?;
            println!("Vault '{name}' -> {}", store.vaults()?[name].display());
        }
        (Some("vault"), Some("list") | None) => {
            for (name, path) in store.vaults()? {
                println!("{name}\t{}", path.display());
            }
        }
        (Some("vault"), Some("rm")) => {
            let name = arg(2).ok_or(Error("usage: scarlet vault rm NAME".into()))?;
            if !store.remove_vault(name)? {
                return Err(Error(format!("no vault named '{name}'")));
            }
        }
        (Some("sync"), Some(vault)) => {
            let vaults = store.vaults()?;
            let root = vaults.get(vault).ok_or(Error(format!("no vault named '{vault}' (see: scarlet vault list)")))?;
            let addr = resolve_addr(&store, args.get(2))?;
            println!("Syncing '{vault}' with {addr} as '{}'…", identity.device_name);
            let summary = run_initiator(&store, connect(&addr)?, Some((vault, root)), &mut Terminal)?;
            store.set_last_addr(&addr)?;
            println!("{}", summary.describe());
            for conflict in &summary.conflicts {
                println!("Conflict: both versions of {conflict} were kept");
            }
        }
        (Some("serve"), port) => {
            let port = match port {
                Some(port) => port.parse().map_err(|_| Error(format!("bad port '{port}'")))?,
                None => DEFAULT_PORT,
            };
            serve(&store, port)?;
        }
        (Some("pair"), addr) => {
            let addr = resolve_addr(&store, addr.map(str::to_string).as_ref())?;
            let summary = run_initiator(&store, connect(&addr)?, None, &mut Terminal)?;
            store.set_last_addr(&addr)?;
            println!("{}", summary.describe());
        }
        (Some("peers"), _) => {
            for peer in store.peers()? {
                println!("{}\t{}", peer.name, fingerprint(&peer.public_key));
            }
        }
        (Some("unpair"), Some(name)) => {
            if !store.remove_peer(name)? {
                return Err(Error(format!("no paired device named '{name}'")));
            }
        }
        _ => println!("{USAGE}"),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

//! Two-way vault sync between two paired devices.
//!
//! The initiator compares three views of the vault: its own files, the
//! responder's files and the state both agreed on after the previous sync
//! (the "base"). That is enough to tell edits from deletions on either side
//! and to detect conflicts, which are resolved by keeping both versions.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::channel::{Channel, IO_TIMEOUT};
use crate::proto::{Entry, Intro, PairAnswer, Request, Response, PROTOCOL_VERSION};
use crate::store::{BaseState, Store};
use crate::vault::{is_note_name, join_rel, resolve, validate_rel_path, write_atomic};
use crate::{err, to_hex, Error, Result, DEFAULT_PORT};

/// How long to wait for the user on the other device to confirm a pairing.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);
const DIR_SIG: &str = "dir";

/// Hooks into whatever is driving the sync: the phone UI or the terminal.
pub trait SyncUi {
    fn log(&mut self, line: &str);
    /// Shows the pairing code and asks whether it matches the other device.
    fn confirm_pairing(&mut self, peer_name: &str, code: &str) -> bool;
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Summary {
    pub vault: String,
    pub peer: String,
    pub received: usize,
    pub sent: usize,
    pub deleted_here: usize,
    pub deleted_there: usize,
    pub conflicts: Vec<String>,
    pub warnings: Vec<String>,
}

impl Summary {
    pub fn describe(&self) -> String {
        if self.vault.is_empty() {
            return format!("Paired with {}", self.peer);
        }
        let mut text = format!(
            "'{}' synced with {}: {} received, {} sent, {} deleted here, {} deleted there",
            self.vault, self.peer, self.received, self.sent, self.deleted_here, self.deleted_there
        );
        if !self.conflicts.is_empty() {
            text.push_str(&format!(", {} conflict(s) kept as copies", self.conflicts.len()));
        }
        text
    }
}

/// Path -> content signature: "dir" or the hex SHA-256 of the note.
type Sigs = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    MkdirLocal(String),
    MkdirRemote(String),
    Pull(String),
    Push(String),
    DeleteLocal(String),
    DeleteRemote(String),
    RmdirLocal(String),
    RmdirRemote(String),
    /// Both sides changed the note differently: keep both versions.
    Conflict(String),
    /// A note on one side and a directory on the other; left for the user to sort out.
    Skip(String),
}

fn sig(entry: &Entry) -> String {
    if entry.is_dir {
        DIR_SIG.to_string()
    } else {
        to_hex(&entry.hash)
    }
}

fn sigs(entries: &[Entry]) -> Sigs {
    entries.iter().map(|e| (e.path.clone(), sig(e))).collect()
}

/// Lists all directories and notes of a vault, skipping hidden entries,
/// symlinks and files that aren't markdown. Sorted by path.
pub fn scan(root: &Path) -> Result<Vec<Entry>> {
    fn walk(root: &Path, rel: &str, out: &mut Vec<Entry>) -> Result<()> {
        for dir_entry in fs::read_dir(resolve(root, rel)?)? {
            let dir_entry = dir_entry?;
            let Ok(name) = dir_entry.file_name().into_string() else { continue };
            let file_type = dir_entry.file_type()?;
            if name.starts_with('.') || file_type.is_symlink() || validate_rel_path(&name).is_err() {
                continue;
            }
            let path = join_rel(rel, &name);
            if file_type.is_dir() {
                out.push(Entry { path: path.clone(), is_dir: true, hash: [0; 32], mtime: 0 });
                walk(root, &path, out)?;
            } else if file_type.is_file() && is_note_name(&name) {
                let data = fs::read(dir_entry.path())?;
                let mtime = dir_entry
                    .metadata()?
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                out.push(Entry { path, is_dir: false, hash: Sha256::digest(&data).into(), mtime });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, "", &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The three-way merge: decides what to do for every path known to either
/// side. Pure, so it can be tested without any I/O.
pub fn plan(local: &Sigs, remote: &Sigs, base: &BaseState) -> Vec<Action> {
    let paths: BTreeSet<&String> = local.keys().chain(remote.keys()).chain(base.keys()).collect();
    let mut actions = Vec::new();
    for path in paths {
        let (l, r, b) = (local.get(path), remote.get(path), base.get(path));
        let is_dir = |s: Option<&String>| s.is_some_and(|s| s == DIR_SIG);
        let p = path.clone();
        if l == r {
            continue;
        }
        let action = if l == b {
            // Only the remote side changed.
            match r {
                None if is_dir(l) => Action::RmdirLocal(p),
                None => Action::DeleteLocal(p),
                Some(_) if l.is_some() && is_dir(l) != is_dir(r) => Action::Skip(p),
                Some(_) if is_dir(r) => Action::MkdirLocal(p),
                Some(_) => Action::Pull(p),
            }
        } else if r == b {
            // Only the local side changed.
            match l {
                None if is_dir(r) => Action::RmdirRemote(p),
                None => Action::DeleteRemote(p),
                Some(_) if r.is_some() && is_dir(l) != is_dir(r) => Action::Skip(p),
                Some(_) if is_dir(l) => Action::MkdirRemote(p),
                Some(_) => Action::Push(p),
            }
        } else {
            // Both sides changed. An edit always wins over a deletion.
            match (l, r) {
                (None, Some(_)) if is_dir(r) => Action::MkdirLocal(p),
                (None, Some(_)) => Action::Pull(p),
                (Some(_), None) if is_dir(l) => Action::MkdirRemote(p),
                (Some(_), None) => Action::Push(p),
                (Some(_), Some(_)) if is_dir(l) || is_dir(r) => Action::Skip(p),
                _ => Action::Conflict(p),
            }
        };
        actions.push(action);
    }
    // Create directories first (parents sort before children), then move
    // notes around, then delete notes, and remove directories last, deepest first.
    let rank = |a: &Action| match a {
        Action::MkdirLocal(_) | Action::MkdirRemote(_) => 0,
        Action::Pull(_) | Action::Push(_) | Action::Conflict(_) | Action::Skip(_) => 1,
        Action::DeleteLocal(_) | Action::DeleteRemote(_) => 2,
        Action::RmdirLocal(_) | Action::RmdirRemote(_) => 3,
    };
    actions.sort_by_key(rank);
    let first_rmdir = actions.iter().position(|a| rank(a) == 3).unwrap_or(actions.len());
    actions[first_rmdir..].reverse();
    actions
}

pub fn connect(addr: &str) -> Result<TcpStream> {
    let addr = addr.trim();
    let with_port = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{DEFAULT_PORT}") };
    let socket_addr = with_port
        .to_socket_addrs()
        .map_err(|e| Error(format!("bad address '{addr}': {e}")))?
        .next()
        .ok_or(Error(format!("bad address '{addr}'")))?;
    TcpStream::connect_timeout(&socket_addr, Duration::from_secs(8))
        .map_err(|e| Error(format!("can't reach {with_port}: {e}")))
}

/// Handshake, introduction and (when the devices don't know each other yet)
/// pairing. Returns the channel and the peer's device name.
fn establish(store: &Store, stream: TcpStream, initiator: bool, ui: &mut dyn SyncUi) -> Result<(Channel, String)> {
    let identity = store.identity()?;
    let mut channel = Channel::handshake(stream, &identity.private_key_bytes()?, initiator)?;
    let known = store.find_peer(channel.remote_key())?;

    channel.send(&Intro {
        version: PROTOCOL_VERSION,
        device_name: identity.device_name.clone(),
        knows_peer: known.is_some(),
    })?;
    let intro: Intro = channel.recv()?;
    if intro.version != PROTOCOL_VERSION {
        return err(format!("incompatible protocol version {} (this device speaks {PROTOCOL_VERSION})", intro.version));
    }
    let peer_name: String = intro.device_name.chars().filter(|c| !c.is_control()).take(64).collect();

    if known.is_none() || !intro.knows_peer {
        let code = channel.pairing_code().to_string();
        let accepted = ui.confirm_pairing(&peer_name, &code);
        channel.send(&PairAnswer(accepted))?;
        if !accepted {
            return err("pairing rejected");
        }
        ui.log("Waiting for confirmation on the other device…");
        channel.set_timeout(PAIRING_TIMEOUT)?;
        let PairAnswer(peer_accepted) = channel.recv()?;
        channel.set_timeout(IO_TIMEOUT)?;
        if !peer_accepted {
            return err("pairing rejected on the other device");
        }
        store.add_peer(&peer_name, channel.remote_key())?;
        ui.log(&format!("Paired with {peer_name}"));
    }
    Ok((channel, peer_name))
}

fn request(channel: &mut Channel, req: &Request) -> Result<Response> {
    channel.send(req)?;
    match channel.recv()? {
        Response::Error(msg) => err(format!("peer: {msg}")),
        response => Ok(response),
    }
}

fn expect_ok(channel: &mut Channel, req: &Request) -> Result<()> {
    match request(channel, req)? {
        Response::Ok => Ok(()),
        other => err(format!("unexpected reply: {other:?}")),
    }
}

fn remote_manifest(channel: &mut Channel) -> Result<Vec<Entry>> {
    let Response::Manifest(entries) = request(channel, &Request::Manifest)? else {
        return err("unexpected reply to manifest request");
    };
    for entry in &entries {
        validate_rel_path(&entry.path)?;
    }
    Ok(entries)
}

fn check_note_path(path: &str) -> Result<()> {
    validate_rel_path(path)?;
    if !is_note_name(path.rsplit('/').next().unwrap_or("")) {
        return err(format!("'{path}' is not a markdown note"));
    }
    Ok(())
}

fn read_local(root: &Path, path: &str) -> Result<(Vec<u8>, u64)> {
    let full = resolve(root, path)?;
    let data = fs::read(&full)?;
    let mtime = fs::metadata(&full)?
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Ok((data, mtime))
}

fn write_local(root: &Path, path: &str, data: &[u8], mtime: u64) -> Result<()> {
    check_note_path(path)?;
    let full = resolve(root, path)?;
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(&full, data)?;
    if mtime > 0 {
        let file = fs::OpenOptions::new().write(true).open(&full)?;
        let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(mtime));
    }
    Ok(())
}

/// "note.md" -> "note.conflict-<device>-<utc timestamp>.md"
fn conflict_path(path: &str, device: &str, now: u64) -> String {
    let device: String =
        device.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '-' }).take(24).collect();
    let stem = &path[..path.len() - 3];
    format!("{stem}.conflict-{device}-{}.md", utc_stamp(now))
}

/// Formats Unix seconds as "YYYYMMDD-HHMMSS" (UTC).
fn utc_stamp(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Connects the dots for the side that starts the sync. With `vault == None`
/// the session only pairs the two devices.
pub fn run_initiator(
    store: &Store,
    stream: TcpStream,
    vault: Option<(&str, &Path)>,
    ui: &mut dyn SyncUi,
) -> Result<Summary> {
    let (mut channel, peer) = establish(store, stream, true, ui)?;
    let mut summary = Summary { peer: peer.clone(), ..Summary::default() };
    let Some((vault, root)) = vault else {
        channel.send(&Request::Bye)?;
        return Ok(summary);
    };
    summary.vault = vault.to_string();
    let peer_key = channel.remote_key().to_vec();

    expect_ok(&mut channel, &Request::Sync { vault: vault.to_string() })?;
    ui.log("Comparing notes…");
    let local = sigs(&scan(root)?);
    let remote_entries = remote_manifest(&mut channel)?;
    let remote_mtimes: BTreeMap<&str, u64> = remote_entries.iter().map(|e| (e.path.as_str(), e.mtime)).collect();
    let base = store.base_state(vault, &peer_key)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());

    for action in plan(&local, &sigs(&remote_entries), &base) {
        match action {
            Action::MkdirLocal(path) => fs::create_dir_all(resolve(root, &path)?)?,
            Action::MkdirRemote(path) => expect_ok(&mut channel, &Request::Mkdir(path))?,
            Action::Pull(path) => {
                ui.log(&format!("↓ {path}"));
                let Response::File { data, mtime } = request(&mut channel, &Request::Get(path.clone()))? else {
                    return err("unexpected reply to file request");
                };
                write_local(root, &path, &data, mtime)?;
                summary.received += 1;
            }
            Action::Push(path) => {
                ui.log(&format!("↑ {path}"));
                let (data, mtime) = read_local(root, &path)?;
                expect_ok(&mut channel, &Request::Put { path, data, mtime })?;
                summary.sent += 1;
            }
            Action::Conflict(path) => {
                // The local version keeps its name; the remote version is
                // stored next to it on both devices.
                let copy = conflict_path(&path, &peer, now);
                ui.log(&format!("! conflict: {path} (other version kept as {copy})"));
                let Response::File { data, .. } = request(&mut channel, &Request::Get(path.clone()))? else {
                    return err("unexpected reply to file request");
                };
                let mtime = remote_mtimes.get(path.as_str()).copied().unwrap_or(0);
                write_local(root, &copy, &data, mtime)?;
                expect_ok(&mut channel, &Request::Put { path: copy.clone(), data, mtime })?;
                let (data, mtime) = read_local(root, &path)?;
                expect_ok(&mut channel, &Request::Put { path: path.clone(), data, mtime })?;
                summary.conflicts.push(path);
            }
            Action::DeleteLocal(path) => {
                ui.log(&format!("× {path}"));
                check_note_path(&path)?;
                fs::remove_file(resolve(root, &path)?)?;
                summary.deleted_here += 1;
            }
            Action::DeleteRemote(path) => {
                ui.log(&format!("× {path} (on {peer})"));
                expect_ok(&mut channel, &Request::Delete(path))?;
                summary.deleted_there += 1;
            }
            // Directories are only removed when empty; if something that
            // isn't synced still lives there, the directory simply stays.
            Action::RmdirLocal(path) => {
                let _ = fs::remove_dir(resolve(root, &path)?);
            }
            Action::RmdirRemote(path) => {
                let _ = request(&mut channel, &Request::Rmdir(path));
            }
            Action::Skip(path) => {
                let warning = format!("'{path}' is a note on one device and a directory on the other; skipped");
                ui.log(&format!("! {warning}"));
                summary.warnings.push(warning);
            }
        }
    }

    // The new base is everything both sides now agree on.
    let local = sigs(&scan(root)?);
    let remote = sigs(&remote_manifest(&mut channel)?);
    let base: BaseState = local.into_iter().filter(|(path, sig)| remote.get(path) == Some(sig)).collect();
    store.save_base_state(vault, &peer_key, &base)?;
    expect_ok(&mut channel, &Request::Done { base })?;
    Ok(summary)
}

/// Serves one incoming session. `resolve_vault` maps a requested vault name
/// to its directory (and may refuse or create it).
pub fn run_responder(
    store: &Store,
    stream: TcpStream,
    resolve_vault: &dyn Fn(&str) -> Result<PathBuf>,
    ui: &mut dyn SyncUi,
) -> Result<Summary> {
    let (mut channel, peer) = establish(store, stream, false, ui)?;
    let mut summary = Summary { peer: peer.clone(), ..Summary::default() };
    let peer_key = channel.remote_key().to_vec();
    let mut root: Option<PathBuf> = None;

    loop {
        let req: Request = channel.recv()?;
        let done = matches!(req, Request::Done { .. });
        let response = match serve(store, &peer_key, &mut root, &mut summary, resolve_vault, ui, req) {
            Ok(Some(response)) => response,
            Ok(None) => return Ok(summary),
            Err(e) => Response::Error(e.0),
        };
        channel.send(&response)?;
        if done {
            return Ok(summary);
        }
    }
}

fn serve(
    store: &Store,
    peer_key: &[u8],
    root: &mut Option<PathBuf>,
    summary: &mut Summary,
    resolve_vault: &dyn Fn(&str) -> Result<PathBuf>,
    ui: &mut dyn SyncUi,
    req: Request,
) -> Result<Option<Response>> {
    let vault_root = || root.clone().ok_or(Error("no vault selected".into()));
    Ok(Some(match req {
        Request::Bye => return Ok(None),
        Request::Sync { vault } => {
            crate::vault::validate_vault_name(&vault)?;
            *root = Some(resolve_vault(&vault)?);
            ui.log(&format!("{} is syncing '{vault}'…", summary.peer));
            summary.vault = vault;
            Response::Ok
        }
        Request::Manifest => Response::Manifest(scan(&vault_root()?)?),
        Request::Get(path) => {
            check_note_path(&path)?;
            let (data, mtime) = read_local(&vault_root()?, &path)?;
            ui.log(&format!("↑ {path}"));
            summary.sent += 1;
            Response::File { data, mtime }
        }
        Request::Put { path, data, mtime } => {
            write_local(&vault_root()?, &path, &data, mtime)?;
            ui.log(&format!("↓ {path}"));
            summary.received += 1;
            Response::Ok
        }
        Request::Delete(path) => {
            check_note_path(&path)?;
            fs::remove_file(resolve(&vault_root()?, &path)?)?;
            ui.log(&format!("× {path}"));
            summary.deleted_here += 1;
            Response::Ok
        }
        Request::Mkdir(path) => {
            validate_rel_path(&path)?;
            fs::create_dir_all(resolve(&vault_root()?, &path)?)?;
            Response::Ok
        }
        Request::Rmdir(path) => {
            validate_rel_path(&path)?;
            fs::remove_dir(resolve(&vault_root()?, &path)?)?;
            Response::Ok
        }
        Request::Done { base } => {
            vault_root()?;
            store.save_base_state(&summary.vault, peer_key, &base)?;
            Response::Ok
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    struct TestUi(bool);

    impl SyncUi for TestUi {
        fn log(&mut self, _line: &str) {}
        fn confirm_pairing(&mut self, _peer_name: &str, _code: &str) -> bool {
            self.0
        }
    }

    struct Device {
        _tmp: tempfile::TempDir,
        store: Store,
        vault: PathBuf,
    }

    fn device(name: &str) -> Device {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("store")).unwrap();
        store.identity_or_create(name).unwrap();
        let vault = tmp.path().join("vault");
        fs::create_dir(&vault).unwrap();
        Device { _tmp: tmp, store, vault }
    }

    /// Runs one sync session: `a` initiates, `b` responds.
    fn sync_with(a: &Device, b: &Device, a_accepts: bool, b_accepts: bool) -> (Result<Summary>, Result<Summary>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (b_store, b_vault) = (b.store.clone(), b.vault.clone());
        let responder = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            run_responder(&b_store, stream, &|_| Ok(b_vault.clone()), &mut TestUi(b_accepts))
        });
        let initiated =
            run_initiator(&a.store, connect(&addr).unwrap(), Some(("notes", &a.vault)), &mut TestUi(a_accepts));
        (initiated, responder.join().unwrap())
    }

    fn sync(a: &Device, b: &Device) -> Summary {
        let (initiated, responded) = sync_with(a, b, true, true);
        responded.unwrap();
        initiated.unwrap()
    }

    fn write(dev: &Device, path: &str, text: &str) {
        let full = dev.vault.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }

    fn read(dev: &Device, path: &str) -> Option<String> {
        fs::read_to_string(dev.vault.join(path)).ok()
    }

    #[test]
    fn plans_three_way_merge() {
        let s = |pairs: &[(&str, &str)]| -> Sigs { pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        let base = s(&[("same.md", "1"), ("l-edit.md", "1"), ("r-edit.md", "1"), ("l-del.md", "1"), ("r-del.md", "1"), ("both.md", "1"), ("gone", "dir")]);
        let local = s(&[("same.md", "1"), ("l-edit.md", "2"), ("r-edit.md", "1"), ("r-del.md", "1"), ("both.md", "2"), ("l-new.md", "9"), ("newdir", "dir")]);
        let remote = s(&[("same.md", "1"), ("l-edit.md", "1"), ("r-edit.md", "3"), ("l-del.md", "1"), ("both.md", "3"), ("r-new.md", "8"), ("gone", "dir")]);
        let actions = plan(&local, &remote, &base);
        let p = |s: &str| s.to_string();
        assert_eq!(
            actions,
            vec![
                Action::MkdirRemote(p("newdir")),
                Action::Conflict(p("both.md")),
                Action::Push(p("l-edit.md")),
                Action::Push(p("l-new.md")),
                Action::Pull(p("r-edit.md")),
                Action::Pull(p("r-new.md")),
                Action::DeleteRemote(p("l-del.md")),
                Action::DeleteLocal(p("r-del.md")),
                Action::RmdirRemote(p("gone")),
            ]
        );
    }

    #[test]
    fn formats_utc_stamp() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(1_790_769_845), "20260930-120405");
        assert_eq!(conflict_path("a/b.md", "My Phone", 0), "a/b.conflict-My-Phone-19700101-000000.md");
    }

    #[test]
    fn full_sync_roundtrip() {
        let (phone, desktop) = (device("phone"), device("desktop"));
        write(&desktop, "inbox.md", "hello");
        write(&desktop, "work/plan.md", "# Plan");
        fs::create_dir(desktop.vault.join("empty")).unwrap();
        fs::write(desktop.vault.join("work/photo.png"), "not synced").unwrap();
        write(&phone, "phone-only.md", "from phone");

        // First sync pairs the devices and merges both sides.
        let summary = sync(&phone, &desktop);
        assert_eq!((summary.received, summary.sent), (2, 1));
        assert_eq!(read(&phone, "work/plan.md").as_deref(), Some("# Plan"));
        assert_eq!(read(&desktop, "phone-only.md").as_deref(), Some("from phone"));
        assert!(phone.vault.join("empty").is_dir());
        assert!(!phone.vault.join("work/photo.png").exists());
        assert_eq!(phone.store.peers().unwrap()[0].name, "desktop");
        assert_eq!(desktop.store.peers().unwrap()[0].name, "phone");

        // Nothing changed: nothing to do.
        let summary = sync(&desktop, &phone);
        assert_eq!((summary.received, summary.sent, summary.deleted_here, summary.deleted_there), (0, 0, 0, 0));

        // Edits and deletions travel in both directions, whoever initiates.
        write(&phone, "inbox.md", "hello from phone");
        fs::remove_file(desktop.vault.join("phone-only.md")).unwrap();
        fs::remove_dir(phone.vault.join("empty")).unwrap();
        let summary = sync(&desktop, &phone);
        assert_eq!((summary.received, summary.deleted_there), (1, 1));
        assert_eq!(read(&desktop, "inbox.md").as_deref(), Some("hello from phone"));
        assert_eq!(read(&phone, "phone-only.md"), None);
        assert!(!desktop.vault.join("empty").exists());

        // Conflicting edits keep both versions on both devices.
        write(&phone, "inbox.md", "phone version");
        write(&desktop, "inbox.md", "desktop version");
        let summary = sync(&phone, &desktop);
        assert_eq!(summary.conflicts, vec!["inbox.md".to_string()]);
        for dev in [&phone, &desktop] {
            assert_eq!(read(dev, "inbox.md").as_deref(), Some("phone version"));
            let copy = fs::read_dir(&dev.vault)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .find(|n| n.starts_with("inbox.conflict-desktop-"))
                .expect("conflict copy");
            assert_eq!(read(dev, &copy).as_deref(), Some("desktop version"));
        }

        // An edit wins over a deletion of the same note.
        write(&phone, "work/plan.md", "# Plan v2");
        fs::remove_file(desktop.vault.join("work/plan.md")).unwrap();
        sync(&desktop, &phone);
        assert_eq!(read(&desktop, "work/plan.md").as_deref(), Some("# Plan v2"));

        // A directory with unsynced files survives deletion of its notes.
        fs::remove_dir_all(phone.vault.join("work")).unwrap();
        sync(&phone, &desktop);
        assert_eq!(read(&desktop, "work/plan.md"), None);
        assert!(desktop.vault.join("work/photo.png").exists());
    }

    #[test]
    fn rejected_pairing_stops_sync() {
        let (phone, desktop) = (device("phone"), device("desktop"));
        write(&desktop, "secret.md", "secret");
        let (initiated, responded) = sync_with(&phone, &desktop, true, false);
        assert!(initiated.is_err());
        assert!(responded.is_err());
        assert_eq!(read(&phone, "secret.md"), None);
        assert!(phone.store.peers().unwrap().is_empty());
        assert!(desktop.store.peers().unwrap().is_empty());
    }

    #[test]
    fn responder_rejects_escaping_paths() {
        let (phone, desktop) = (device("phone"), device("desktop"));
        sync(&phone, &desktop);
        let mut summary = Summary::default();
        let mut root = Some(desktop.vault.clone());
        let mut call = |req| serve(&desktop.store, &[0; 32], &mut root, &mut summary, &|_| err("no"), &mut TestUi(true), req);
        assert!(call(Request::Put { path: "../evil.md".into(), data: vec![], mtime: 0 }).is_err());
        assert!(call(Request::Put { path: "script.sh".into(), data: vec![], mtime: 0 }).is_err());
        assert!(call(Request::Put { path: ".hidden/x.md".into(), data: vec![], mtime: 0 }).is_err());
        assert!(call(Request::Get("/etc/passwd".into())).is_err());
        assert!(call(Request::Delete("../x.md".into())).is_err());
        assert!(call(Request::Rmdir("..".into())).is_err());
        assert!(call(Request::Put { path: "ok/fine.md".into(), data: b"x".to_vec(), mtime: 0 }).is_ok());
    }
}

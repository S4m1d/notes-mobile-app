//! The sync screen: runs sync sessions on worker threads and reports their
//! progress back to the UI.

use std::cell::RefCell;
use std::fs;
use std::io::ErrorKind;
use std::net::{TcpListener, UdpSocket};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use scarlet_core::sync::{connect, run_initiator, run_responder, Summary, SyncUi};
use scarlet_core::{Result, DEFAULT_PORT};
use slint::{Model, SharedString, VecModel};

use crate::{post, App, DialogKind, Screen};

const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct SyncState {
    pub log: Rc<VecModel<SharedString>>,
    /// Where to send the user's answer to the pairing dialog.
    pairing_answer: RefCell<Option<mpsc::Sender<bool>>>,
    /// Set to make the listener thread stop.
    stop: RefCell<Option<Arc<AtomicBool>>>,
}

impl SyncState {
    pub fn answer_pairing(&self, accepted: bool) {
        if let Some(answer) = self.pairing_answer.borrow_mut().take() {
            let _ = answer.send(accepted);
        }
    }

    pub fn stop_listening(&self) {
        if let Some(stop) = self.stop.borrow_mut().take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn push(&self, line: &str) {
        self.log.push(line.into());
        // Keep the log bounded.
        while self.log.row_count() > 200 {
            self.log.remove(0);
        }
    }
}

/// The worker thread's side of the UI: forwards everything to the UI thread.
struct Bridge;

impl SyncUi for Bridge {
    fn log(&mut self, line: &str) {
        let line = line.to_string();
        post(move |app| app.sync.push(&line));
    }

    fn confirm_pairing(&mut self, peer_name: &str, code: &str) -> bool {
        let (answer, response) = mpsc::channel();
        let (peer_name, code) = (peer_name.to_string(), code.to_string());
        post(move |app| {
            *app.sync.pairing_answer.borrow_mut() = Some(answer);
            app.ui.set_dialog_subject(peer_name.into());
            app.ui.set_dialog_detail(code.into());
            app.ui.set_dialog(DialogKind::Pairing);
        });
        response.recv_timeout(PAIRING_TIMEOUT).unwrap_or(false)
    }
}

/// This device's address on the local network, found without sending
/// anything: connecting a UDP socket only selects the outgoing interface.
fn local_address() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

impl App {
    pub(crate) fn show_device(&self) {
        if let Some(identity) = self.report(self.store.identity()) {
            self.ui.set_device_key(identity.fingerprint().into());
            self.ui.set_device_name(identity.device_name.into());
        }
        self.ui.set_device_address(local_address().unwrap_or_else(|| "no network".into()).into());
    }

    pub(crate) fn open_sync(&self) {
        self.sync_origin.set(self.ui.get_screen());
        self.show_device();
        self.ui.set_sync_address(self.store.last_addr().into());
        self.sync.log.set_vec(Vec::new());
        self.ui.set_screen(Screen::Sync);
    }

    /// Syncs the open vault with a desktop running `scarlet serve`.
    pub(crate) fn sync_connect(&self, address: &str) {
        let address = address.trim().to_string();
        let vault = self.vault.borrow().clone();
        if address.is_empty() || vault.is_empty() || self.ui.get_sync_busy() || self.ui.get_sync_listening() {
            return;
        }
        self.ui.invoke_grab_focus();
        self.ui.set_sync_busy(true);
        self.sync.push(&format!("Connecting to {address}…"));
        let (store, root) = (self.store.clone(), self.vault_root());
        std::thread::spawn(move || {
            let result = connect(&address)
                .and_then(|stream| run_initiator(&store, stream, Some((&vault, &root)), &mut Bridge));
            if result.is_ok() {
                let _ = store.set_last_addr(&address);
            }
            post(move |app| app.session_finished(result));
        });
    }

    /// Waits for the desktop to connect (`scarlet sync VAULT ADDRESS`).
    /// Vaults the phone doesn't have yet are created on the fly.
    pub(crate) fn sync_listen(&self) {
        if self.ui.get_sync_busy() || self.ui.get_sync_listening() {
            return;
        }
        let bound = TcpListener::bind(("0.0.0.0", DEFAULT_PORT)).and_then(|listener| {
            listener.set_nonblocking(true)?;
            Ok(listener)
        });
        let Some(listener) = self.report(bound.map_err(Into::into)) else { return };
        let stop = Arc::new(AtomicBool::new(false));
        *self.sync.stop.borrow_mut() = Some(stop.clone());
        self.ui.set_sync_listening(true);
        self.sync.push("Listening… start the sync on the desktop.");

        let (store, vaults_dir) = (self.store.clone(), self.vaults_dir.clone());
        std::thread::spawn(move || {
            let resolve_vault = |name: &str| -> Result<_> {
                let root = vaults_dir.join(name);
                fs::create_dir_all(&root)?;
                Ok(root)
            };
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        post(|app| app.ui.set_sync_busy(true));
                        let result = stream
                            .set_nonblocking(false)
                            .map_err(Into::into)
                            .and_then(|()| run_responder(&store, stream, &resolve_vault, &mut Bridge));
                        post(move |app| app.session_finished(result));
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(150)),
                    Err(e) => {
                        let message = format!("Listening failed: {e}");
                        post(move |app| app.sync.push(&message));
                        break;
                    }
                }
            }
            post(|app| {
                app.ui.set_sync_listening(false);
                app.sync.push("Stopped listening.");
            });
        });
    }

    fn session_finished(&self, result: Result<Summary>) {
        self.ui.set_sync_busy(false);
        if self.ui.get_dialog() == DialogKind::Pairing {
            self.close_dialog();
        }
        match result {
            Ok(summary) => {
                self.sync.push(&format!("✓ {}", summary.describe()));
                for warning in &summary.warnings {
                    self.sync.push(&format!("! {warning}"));
                }
            }
            Err(e) => self.sync.push(&format!("× Failed: {e}")),
        }
    }
}

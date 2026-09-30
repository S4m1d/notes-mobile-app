//! Scarlet Notes: the mobile app. The UI lives in `ui/*.slint`; this crate
//! wires it to the vault, editor and sync logic.

slint::include_modules!();

mod editor;
mod sync_ui;

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use scarlet_core::store::Store;
use scarlet_core::{vault, Error, Result};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

thread_local! {
    /// The running app, for code that gets called without a handle to it:
    /// sync worker threads posting back to the UI thread and Android
    /// lifecycle events.
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Runs `f` on the UI thread with the running app. Callable from any thread.
pub(crate) fn post(f: impl FnOnce(&Rc<App>) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

fn with_app(f: impl FnOnce(&Rc<App>)) {
    let app = APP.with(|app| app.borrow().clone());
    if let Some(app) = app {
        f(&app);
    }
}

pub struct App {
    ui: AppWindow,
    store: Store,
    vaults_dir: PathBuf,
    /// Name of the open vault; empty on the vault list.
    vault: RefCell<String>,
    /// Directory shown by the browser, relative to the vault root.
    dir: RefCell<String>,
    entries: RefCell<Vec<vault::Item>>,
    editor: editor::Editor,
    sync: sync_ui::SyncState,
    /// The screen the sync screen was opened from.
    sync_origin: Cell<Screen>,
    message_timer: Timer,
}

impl App {
    fn vault_root(&self) -> PathBuf {
        self.vaults_dir.join(self.vault.borrow().as_str())
    }

    /// Shows the error in the message bar for a few seconds.
    fn report<T>(&self, result: Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(Error(message)) => {
                self.ui.set_message(message.into());
                let ui = self.ui.as_weak();
                self.message_timer.start(TimerMode::SingleShot, Duration::from_secs(6), move || {
                    if let Some(ui) = ui.upgrade() {
                        ui.set_message("".into());
                    }
                });
                None
            }
        }
    }

    fn show_vaults(&self) {
        let mut names: Vec<String> = fs::read_dir(&self.vaults_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort_by_key(|name| name.to_lowercase());
        let names: Vec<SharedString> = names.into_iter().map(Into::into).collect();
        self.vault.borrow_mut().clear();
        self.dir.borrow_mut().clear();
        self.ui.set_vaults(ModelRc::new(VecModel::from(names)));
        self.ui.set_vault_name("".into());
        self.ui.set_screen(Screen::Vaults);
    }

    fn show_browser(&self) {
        let dir = self.dir.borrow().clone();
        let items = self.report(vault::list(&self.vault_root(), &dir)).unwrap_or_default();
        let rows: Vec<Entry> = items.iter().map(|i| Entry { name: i.name.as_str().into(), is_dir: i.is_dir }).collect();
        *self.entries.borrow_mut() = items;
        self.ui.set_entries(ModelRc::new(VecModel::from(rows)));
        self.ui.set_vault_name(self.vault.borrow().as_str().into());
        self.ui.set_dir_path(dir.into());
        self.ui.set_screen(Screen::Browser);
    }

    fn open_vault(&self, name: &str) {
        *self.vault.borrow_mut() = name.to_string();
        self.dir.borrow_mut().clear();
        self.show_browser();
    }

    fn open_note(&self, rel: &str) {
        if self.report(self.editor.open(&self.ui, self.vault_root(), rel)).is_some() {
            let title = rel.rsplit('/').next().unwrap_or(rel);
            self.ui.set_note_title(title.trim_end_matches(".md").into());
            self.ui.set_screen(Screen::Editor);
        }
    }

    fn open_entry(&self, index: usize) {
        let Some(item) = self.entries.borrow().get(index).cloned() else { return };
        let rel = vault::join_rel(&self.dir.borrow(), &item.name);
        if item.is_dir {
            *self.dir.borrow_mut() = rel;
            self.show_browser();
        } else {
            self.open_note(&rel);
        }
    }

    /// The back arrow and the system back gesture. Returns false when
    /// there is nowhere to go back to, which lets Android close the app.
    fn back(&self) -> bool {
        if self.ui.get_dialog() != DialogKind::None {
            self.dialog_cancelled(self.ui.get_dialog());
            return true;
        }
        match self.ui.get_screen() {
            Screen::Vaults => return false,
            Screen::Browser => {
                let dir = self.dir.borrow().clone();
                if dir.is_empty() {
                    self.show_vaults();
                } else {
                    *self.dir.borrow_mut() = vault::parent_rel(&dir).to_string();
                    self.show_browser();
                }
            }
            Screen::Editor => {
                self.report(self.editor.close(&self.ui));
                self.show_browser();
            }
            Screen::Sync => {
                if self.ui.get_sync_busy() {
                    return true;
                }
                self.sync.stop_listening();
                if self.sync_origin.get() == Screen::Browser {
                    self.show_browser();
                } else {
                    self.show_vaults();
                }
            }
        }
        self.ui.invoke_grab_focus();
        true
    }

    fn close_dialog(&self) {
        self.ui.set_dialog(DialogKind::None);
        self.ui.set_dialog_detail("".into());
        self.ui.set_dialog_initial_text("".into());
        self.ui.invoke_grab_focus();
    }

    fn dialog_confirmed(&self, kind: DialogKind, text: &str) {
        let subject = self.ui.get_dialog_subject().to_string();
        let root = self.vault_root();
        let dir = self.dir.borrow().clone();
        self.close_dialog();
        match kind {
            DialogKind::None => {}
            DialogKind::NewVault => {
                let created = vault::validate_vault_name(text.trim()).and_then(|()| {
                    let path = self.vaults_dir.join(text.trim());
                    if path.exists() {
                        return Err(Error(format!("vault '{}' already exists", text.trim())));
                    }
                    Ok(fs::create_dir_all(path)?)
                });
                self.report(created);
                self.show_vaults();
            }
            DialogKind::NewDir => {
                self.report(vault::create_dir(&root, &dir, text));
                self.show_browser();
            }
            DialogKind::NewNote => {
                if let Some(rel) = self.report(vault::create_note(&root, &dir, text)) {
                    self.show_browser();
                    self.open_note(&rel);
                }
            }
            DialogKind::DeleteVault => {
                let deleted = vault::validate_vault_name(&subject).and_then(|()| {
                    fs::remove_dir_all(self.vaults_dir.join(&subject))?;
                    self.store.forget_vault_state(&subject)
                });
                self.report(deleted);
                self.show_vaults();
            }
            DialogKind::DeleteEntry => {
                self.report(vault::delete(&root, &vault::join_rel(&dir, &subject)));
                self.show_browser();
            }
            DialogKind::Pairing => self.sync.answer_pairing(true),
            DialogKind::DeviceName => {
                self.report(self.store.set_device_name(text));
                self.show_device();
            }
        }
    }

    fn dialog_cancelled(&self, kind: DialogKind) {
        self.close_dialog();
        if kind == DialogKind::Pairing {
            self.sync.answer_pairing(false);
        }
    }

    fn save_note(&self) {
        self.report(self.editor.save());
    }
}

/// Saves the open note; the target of the debounced auto-save.
fn save_later() -> impl FnMut() + 'static {
    || with_app(|app| app.save_note())
}

fn connect_ui(app: &Rc<App>) {
    let logic = app.ui.global::<Logic>();
    // Callbacks hold weak references: the app owns the window, not vice versa.
    macro_rules! on {
        ($setter:ident, |$app:ident $(, $arg:ident)*| $body:expr) => {{
            let weak = Rc::downgrade(app);
            logic.$setter(move |$($arg),*| {
                let $app = weak.upgrade().expect("app outlives its window");
                $body
            });
        }};
    }

    on!(on_back, |app| app.back());
    on!(on_open_vault, |app, name| app.open_vault(&name));
    on!(on_open_entry, |app, index| app.open_entry(index as usize));
    on!(on_open_sync, |app| app.open_sync());
    on!(on_dialog_confirmed, |app, kind, text| app.dialog_confirmed(kind, &text));
    on!(on_dialog_cancelled, |app, kind| app.dialog_cancelled(kind));

    on!(on_line_activated, |app, index| app.editor.line_activated(&app.ui, index as usize));
    on!(on_line_edited, |app, index, text| app.editor.line_edited(&app.ui, index as usize, &text, save_later()));
    on!(on_line_toggled, |app, index| app.editor.line_toggled(index as usize, save_later()));
    on!(on_line_merged_up, |app, index| app.editor.line_merged_up(&app.ui, index as usize, save_later()));
    on!(on_editor_tap_end, |app| app.editor.tap_end(&app.ui));
    on!(on_editor_add_task, |app| app.editor.add_task(&app.ui, save_later()));
    on!(on_editor_done, |app| {
        app.report(app.editor.done(&app.ui));
        app.ui.invoke_grab_focus();
    });

    on!(on_sync_connect, |app, address| app.sync_connect(&address));
    on!(on_sync_listen, |app| app.sync_listen());
    on!(on_sync_stop, |app| app.sync.stop_listening());
}

/// Creates the window and runs the app until it is closed. `data_dir` is
/// the app-private directory holding the vaults and the device keys.
pub fn run(data_dir: PathBuf) -> Result<()> {
    let platform_error = |e: slint::PlatformError| Error(e.to_string());
    let vaults_dir = data_dir.join("vaults");
    fs::create_dir_all(&vaults_dir)?;
    let store = Store::open(data_dir.join("store"))?;
    store.identity_or_create("phone")?;

    let ui = AppWindow::new().map_err(platform_error)?;
    let app = Rc::new(App {
        ui,
        store,
        vaults_dir,
        vault: RefCell::default(),
        dir: RefCell::default(),
        entries: RefCell::default(),
        editor: editor::Editor::default(),
        sync: sync_ui::SyncState::default(),
        sync_origin: Cell::new(Screen::Vaults),
        message_timer: Timer::default(),
    });
    app.ui.set_lines(app.editor.model.clone().into());
    app.ui.set_sync_log(app.sync.log.clone().into());
    connect_ui(&app);
    app.show_vaults();

    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));
    let result = app.ui.run().map_err(platform_error);
    app.save_note();
    APP.with(|slot| slot.borrow_mut().take());
    result
}

/// Called when the app goes to the background: the OS may kill it at any
/// point after that, so the open note is written out now.
pub fn on_pause() {
    with_app(|app| app.save_note());
}

#[cfg(target_os = "android")]
mod android {
    use std::ffi::{c_char, c_int, CString};

    use slint::android::android_activity::{MainEvent, PollEvent};

    #[link(name = "log")]
    extern "C" {
        fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    const ANDROID_LOG_ERROR: c_int = 6;

    fn log_error(message: &str) {
        let text = CString::new(message.replace('\0', " ")).unwrap_or_default();
        // SAFETY: both pointers are valid NUL-terminated strings for the duration of the call.
        unsafe { __android_log_write(ANDROID_LOG_ERROR, c"scarlet-notes".as_ptr(), text.as_ptr()) };
    }

    #[unsafe(no_mangle)]
    fn android_main(app: slint::android::AndroidApp) {
        // Rust panics go to stderr, which Android discards; send them to logcat.
        std::panic::set_hook(Box::new(|info| log_error(&info.to_string())));

        let Some(data_dir) = app.internal_data_path() else {
            log_error("no internal data path");
            return;
        };
        slint::android::init_with_event_listener(app, |event| {
            if let PollEvent::Main(MainEvent::Pause | MainEvent::SaveState { .. }) = event {
                crate::on_pause();
            }
        })
        .expect("Slint platform already initialised");
        if let Err(e) = crate::run(data_dir) {
            log_error(&format!("fatal: {e}"));
        }
    }
}

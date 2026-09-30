# Progress

`[x]` done and verified · `[~]` written, not fully verified · `[ ]` not started

"Verified on the phone" means exercised on the Pixel 9a (GrapheneOS,
Android 17) over adb on 2026-09-30.

## 1. Project setup

- [x] Cargo workspace: `core`, `app`, `cli`
- [x] Android toolchain check (Rust target, NDK, SDK, cargo-ndk)
- [x] APK build without Gradle (`scripts/build-apk.sh`)
- [x] Install and launch on the Pixel 9a

## 2. Vaults and navigation

- [x] Vault file operations with path validation (unit tests)
- [x] Vault list: create (verified on the phone)
- [~] Vault list: delete with confirmation (same dialog as entries; not tapped through)
- [x] Directory browser: open, back, create directory, create note, delete with confirmation
- [x] System back gesture navigates up

## 3. Editor

- [x] Markdown line classification (unit tests)
- [x] Rendered lines; the caret line shows plain markdown
- [x] Enter splits a line, Backspace at line start joins lines (on-screen keyboard)
- [x] Task button inserts `- [ ] `
- [x] Tap a checkbox to toggle it
- [x] Lists continue on Enter; Enter on an empty item ends the list
- [x] Auto-save (after typing pauses, on leaving, on app pause)
- [x] Caret stays visible above the on-screen keyboard
- [ ] Italic text is not slanted (the system font has no italic face)
- [ ] Hardware keyboards type lower-case only (Slint ignores Shift on Android key events)

## 4. Sync

- [x] Device keys and Noise XX encrypted channel (unit tests)
- [x] Pairing with a confirmation code (verified phone <-> desktop CLI)
- [x] Three-way merge with deletions and conflict copies (unit tests, verified on the phone)
- [x] Desktop CLI: `vault`, `sync`, `serve`, `pair`, `peers`
- [x] Phone listens, desktop starts the sync (`scarlet sync`)
- [x] A listening phone creates vaults it doesn't have yet
- [~] Phone connects to `scarlet serve` (same code path; not run against a real desktop yet)
- [ ] List and remove paired devices on the phone

## 5. Polish

- [x] Scarlet Protocol theme, launcher icon
- [x] Release build (7 MB APK)

## Later / ideas

- [ ] Bluetooth transport for sync
- [ ] Rename and move notes and directories
- [ ] Tap position places the caret inside the line
- [ ] Search
- [ ] Desktop address discovery (mDNS) instead of typing the IP
- [ ] Declare Android 17's local-network permission once the target SDK moves to 37
      (today it is granted implicitly because the app targets SDK 36)

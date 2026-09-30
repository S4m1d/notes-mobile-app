# Scarlet Notes — project description

Scarlet Notes is a security-first note-taking app for Android. Notes are
plain markdown files in a directory tree, so the same notes can be edited
with any text editor on any device. The phone and the desktop exchange notes
through an on-demand, end-to-end encrypted sync.

## Principles

- **Security first.** No accounts, no cloud, no analytics, no Google services.
- **No internet required.** The app works fully offline. The network is used
  only for the sync the user starts, and only with a paired device on the
  local network.
- **No fancy features.** Notes, directories, a markdown editor and sync.
- **Plain files.** A note is a `.md` file, a category is a directory. No
  database and no metadata inside the vault.

## Target devices

- Android, including GrapheneOS. The reference device is a Pixel 9a running
  GrapheneOS (arm64, Android 11+ required).
- A Linux desktop, which takes part in the sync through a small CLI tool.

## Tech stack

- **Rust** for all app, sync and CLI code.
- **Slint** for the UI.
- Caveat on "pure Rust": this project contains no Java, Kotlin or C code,
  but Slint's Android backend ships a small Java helper (on-screen keyboard
  integration) and renders with Skia, a C++ library. Both come with Slint.
- Cryptography: the Noise protocol (`snow` crate with pure-Rust primitives).
- No Gradle: the APK is assembled by `scripts/build-apk.sh` from `cargo-ndk`
  output with the Android SDK command-line tools.

## How it is used

The user already keeps notes on a Linux desktop: a directory with a tree of
sub-directories for categories, edited with Neovim and other editors. The
same notes should be available on the phone:

- Notes created or changed on either device reach the other one by sync.
- Because notes are plain markdown files, anything on the desktop can edit them.
- On the phone, notes are edited with the built-in editor.

## Notes storage

- Markdown (`.md`) files only, stored as plain files in a directory tree.
- The app supports several independent **vaults**. A vault is a top-level
  directory with its own tree of directories and notes.
- On the phone, vaults live in the app's private storage. No storage
  permission is needed and no other app can read the notes; they leave the
  phone only through sync. (Trade-off, chosen deliberately: other editor apps
  on the phone can't open the files.)
- Hidden files and directories (names starting with a dot) and non-markdown
  files are ignored by the app and by sync.

## Navigation and control

- The start screen lists the vaults. Vaults can be created and deleted.
- Inside a vault the user can:
  - add a directory in any directory;
  - add a note in any directory;
  - delete any directory or note, after a confirmation dialog;
  - open a directory by tapping it, and go back up with the back arrow or
    the system back gesture.

## UI style

- Dark theme only, minimalistic.
- Colours follow the Ghostty theme *Cyberpunk Scarlet Protocol*: a
  near-black background (`#101116`), scarlet accents (`#e41951`), mint green
  for the caret and completed tasks (`#76ff9f`, `#01dc84`), cyan for notes
  and links (`#00c5c7`).

## Note editing and appearance

- A note is shown as rendered markdown.
- The line holding the caret is shown as plain markdown source, so it is
  easy to edit. As soon as the caret leaves a line, the line is rendered again.
- Rendered per line: headings, bullet and numbered lists (with nesting),
  task checkboxes, block quotes, fenced code blocks, horizontal rules, and
  inline bold, italic, strikethrough, inline code and links.
- Notes are saved automatically: shortly after typing pauses, when leaving
  the note and when the app goes to the background.

### Editing helpers

- A **task button** in the editor toolbar starts a new task line (`- [ ] `)
  and leaves the caret after it, ready for the task description.
- Tapping a rendered checkbox ticks or unticks the task.
- Pressing Enter in a list or task list continues the list; pressing Enter
  on an empty item ends it.

## Sync

- Each vault is synced independently of the others. Vaults are matched by name.
- Sync is on demand and can be started from either side:
  - from the phone, connecting to the desktop running `scarlet serve`;
  - from the desktop with `scarlet sync VAULT ADDRESS`, while the phone is
    listening on its sync screen.
- Transport: TCP on the local network (Wi-Fi or the phone's hotspot).
  Bluetooth is a possible later addition; the protocol doesn't depend on TCP.
- **End-to-end encryption:** every device creates a static key pair once.
  Each connection runs a Noise XX handshake with those keys
  (`Noise_XX_25519_ChaChaPoly_BLAKE2s`), so all traffic is encrypted and
  mutually authenticated.
- **Pairing:** the first time two devices connect, both show a short code
  derived from the handshake. The user confirms on both devices that the
  codes match; after that the devices trust each other's keys. Unknown
  devices can't sync.
- **Two-way merge:** sync compares both sides with the state of the last
  sync, so edits and deletions travel in both directions.
- **Conflicts:** when a note was changed on both devices, both versions are
  kept. One stays in place, the other is saved next to it as
  `name.conflict-<device>-<date>.md`. An edit always wins over a deletion.
- A paired device can only touch markdown files inside the vault being
  synced; paths are validated on the receiving side.

## Desktop CLI

`scarlet` is a small command-line tool for the Linux desktop:

- registers existing note directories as vaults (`scarlet vault add`);
- syncs a vault with the phone (`scarlet sync`) or waits for the phone to
  start a sync (`scarlet serve`);
- manages the device identity and paired devices.

## Android permissions

- `INTERNET` — required for any socket, including local-network sync. The
  app never contacts anything but the paired device. On GrapheneOS the
  Network permission can stay revoked except while syncing.
- Nothing else.

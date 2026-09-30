# Scarlet Notes

A security-first markdown notes app for Android (GrapheneOS friendly) with an
end-to-end encrypted, on-demand sync to a Linux desktop. Written in Rust with
a Slint UI.

- What the app is and does: [PROJECT.md](PROJECT.md)
- Progress and planned features: [TODO.md](TODO.md)

## Layout

| Path | What |
| --- | --- |
| `crates/core` | Vault file operations, markdown line classification, device keys, sync protocol. Shared by the app and the CLI. |
| `crates/app` | The Android app: Slint UI in `ui/`, glue code in `src/`. |
| `crates/cli` | `scarlet`, the desktop sync tool. |
| `android/` | Manifest and resources for the APK. |
| `scripts/build-apk.sh` | Builds and signs the APK without Gradle. |

## Requirements

- Rust with the `aarch64-linux-android` target, and `cargo-ndk`
- Android SDK (platform + build-tools) and NDK, with `ANDROID_HOME` and
  `ANDROID_NDK_HOME` set
- A JDK (`javac`, `keytool`) with `JAVA_HOME` set
- `adb` for installing on the phone

## Building

```sh
cargo test -p scarlet-core          # core logic and sync tests
cargo build --release -p scarlet-cli    # desktop tool: target/release/scarlet

scripts/build-apk.sh                # debug APK -> build/scarlet-notes.apk
scripts/build-apk.sh --release      # optimised APK
scripts/build-apk.sh --install      # build, install over adb and launch
```

The first build creates a signing key in `android/debug.keystore`. Keep it:
Android only updates an installed app when the key stays the same, and
uninstalling the app deletes the notes stored on the phone.

## Syncing with the desktop

On the desktop, once:

```sh
scarlet vault add notes ~/notes     # register an existing notes directory
```

Then either start the sync from the desktop — open the sync screen on the
phone, tap **Listen**, and run:

```sh
scarlet sync notes 192.168.1.23     # the address shown on the phone
```

or start it from the phone — run `scarlet serve` on the desktop, open the
vault on the phone, tap the sync icon, enter the desktop's address and tap
**Sync**. The default port is 47800; it must be reachable through the
desktop's firewall for `scarlet serve`.

The first connection between two devices asks on both sides to confirm a
pairing code. Run `scarlet` without arguments for all commands.

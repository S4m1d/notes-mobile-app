# Vendored crates

## i-slint-backend-android-activity (1.18.1)

A copy of the crates.io release with one change, marked "Scarlet Notes patch"
in `androidwindowadapter.rs`: the on-screen keyboard is no longer hidden when
the window merely loses focus. Without it, opening the keyboard's language
picker (long-press on space) closed the keyboard.

When updating Slint, replace the copy with the matching release and re-apply
the patch, or drop the `[patch.crates-io]` entry if upstream has fixed it.

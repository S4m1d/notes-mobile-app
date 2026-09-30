#!/usr/bin/env bash
# Builds and signs the Android APK without Gradle:
#   cargo-ndk -> aapt2 (manifest + resources) -> zipalign -> apksigner
#
# Usage: scripts/build-apk.sh [--release] [--install]
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE=debug
CARGO_FLAGS=()
INSTALL=false
for arg in "$@"; do
    case "$arg" in
        --release) PROFILE=release; CARGO_FLAGS+=(--release) ;;
        --install) INSTALL=true ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

: "${ANDROID_HOME:?ANDROID_HOME must point to the Android SDK}"
: "${ANDROID_NDK_HOME:?ANDROID_NDK_HOME must point to the Android NDK}"
MIN_SDK=30
ABI=arm64-v8a
TRIPLE=aarch64-linux-android
LIB=libscarlet_notes.so

BUILD_TOOLS=$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)
PLATFORM=$(ls -d "$ANDROID_HOME"/platforms/android-* | sort -V | tail -1)
TARGET_SDK=${PLATFORM##*-}
# Slint's build script needs android.jar to compile its small Java helper.
export ANDROID_JAR="$PLATFORM/android.jar"

cargo ndk --target "$ABI" --platform "$MIN_SDK" build -p scarlet-notes "${CARGO_FLAGS[@]}"

OUT=build/apk
rm -rf "$OUT"
mkdir -p "$OUT/lib/$ABI"
"$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip \
    -o "$OUT/lib/$ABI/$LIB" "target/$TRIPLE/$PROFILE/$LIB"

"$BUILD_TOOLS/aapt2" compile --dir android/res -o "$OUT/res.zip"
"$BUILD_TOOLS/aapt2" link -o "$OUT/unaligned.apk" \
    --manifest android/AndroidManifest.xml -I "$ANDROID_JAR" \
    --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
    "$OUT/res.zip"
(cd "$OUT" && "$BUILD_TOOLS/aapt" add unaligned.apk "lib/$ABI/$LIB" > /dev/null)
"$BUILD_TOOLS/zipalign" -f 4 "$OUT/unaligned.apk" "$OUT/aligned.apk"

# The signing key must stay the same between builds, otherwise Android
# refuses to update the installed app (and uninstalling deletes the notes).
KEYSTORE=${SCARLET_KEYSTORE:-android/debug.keystore}
if [ ! -f "$KEYSTORE" ]; then
    keytool -genkeypair -keystore "$KEYSTORE" -storepass android -keypass android \
        -alias scarlet -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=Scarlet Notes" > /dev/null 2>&1
fi
APK=build/scarlet-notes.apk
"$BUILD_TOOLS/apksigner" sign --ks "$KEYSTORE" --ks-pass pass:android --key-pass pass:android \
    --out "$APK" "$OUT/aligned.apk"
echo "Built $APK ($(du -h "$APK" | cut -f1))"

if $INSTALL; then
    adb install -r "$APK"
    adb shell am start -n org.scarletnotes.app/android.app.NativeActivity
fi

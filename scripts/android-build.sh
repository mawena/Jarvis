#!/usr/bin/env bash
# Construit l'appli Android de l'oreille (Pixel 4 XL, arm64-v8a seulement) :
# bibliothèque Rust par cargo-ndk, liaisons Kotlin par uniffi-bindgen, APK par Gradle.
#
#   scripts/android-build.sh [--test] [--offline]
#
#   --test     lance aussi les tests Kotlin en JVM (testDebugUnitTest)
#   --offline  Gradle sans réseau (après un premier passage en ligne)
#
# Variables : ANDROID_HOME (défaut ~/Android), ANDROID_NDK_HOME (défaut : NDK 28.2 de
# ANDROID_HOME), JARVIS_JAVA_HOME (sinon JDK 21, sinon 17), CARGO_TARGET_DIR (sinon le
# dossier target du dépôt principal, même depuis un worktree).
set -euo pipefail

NDK_VERSION="28.2.13676358"
RUST_TARGET="aarch64-linux-android"
ABI="arm64-v8a"
API=33

usage() {
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
}

test_task=""
offline=""
for arg in "$@"; do
    case "$arg" in
        --test) test_task="testDebugUnitTest" ;;
        --offline) offline="--offline" ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "option inconnue : $arg" >&2
            usage >&2
            exit 2
            ;;
    esac
done

fail() {
    echo "erreur : $*" >&2
    exit 1
}

repo="$(cd "$(dirname "$0")/.." && pwd)"
app="$repo/apps/android"

# 1. Outils Android et Rust.
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android}"
[[ -d "$ANDROID_HOME/platforms" ]] \
    || fail "SDK Android introuvable dans $ANDROID_HOME (réglez ANDROID_HOME)"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/$NDK_VERSION}"
[[ -d "$ANDROID_NDK_HOME/toolchains" ]] \
    || fail "NDK $NDK_VERSION introuvable dans $ANDROID_NDK_HOME : sdkmanager \"ndk;$NDK_VERSION\""
rustup target list --installed | grep -qx "$RUST_TARGET" \
    || fail "cible Rust $RUST_TARGET absente : rustup target add $RUST_TARGET"
cargo ndk --version >/dev/null 2>&1 \
    || fail "cargo-ndk absent : cargo install cargo-ndk"

# 2. JDK pour Gradle (le JDK par défaut peut être trop récent pour le plugin Android).
pick_java() {
    local candidate
    for candidate in "${JARVIS_JAVA_HOME:-}" \
        /usr/lib/jvm/java-21-openjdk-amd64 /usr/lib/jvm/java-21-openjdk \
        /usr/lib/jvm/java-17-openjdk-amd64 /usr/lib/jvm/java-17-openjdk; do
        if [[ -n "$candidate" && -x "$candidate/bin/java" ]]; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}
JAVA_HOME="$(pick_java)" \
    || fail "JDK 21 ou 17 introuvable (sudo apt install openjdk-21-jdk-headless, ou JARVIS_JAVA_HOME)"
export JAVA_HOME

# 3. Dossier de compilation de cargo : celui du dépôt principal (jamais un target par worktree).
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
    common="$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
    if [[ -n "$common" ]]; then
        CARGO_TARGET_DIR="$(dirname "$common")/target"
    else
        CARGO_TARGET_DIR="$repo/target"
    fi
fi
export CARGO_TARGET_DIR

cd "$repo"

# 4. Bibliothèque Rust pour le téléphone.
echo "==> bibliothèque Rust ($ABI, API $API)"
rm -rf "$app/app/build/rustJniLibs"
cargo ndk -t "$ABI" -P "$API" -o "$app/app/build/rustJniLibs" \
    build --release -p jarvis-ear-ffi

# 5. Liaisons Kotlin, lues dans la bibliothèque non dépouillée (métadonnées UniFFI).
echo "==> liaisons Kotlin"
lib="$CARGO_TARGET_DIR/$RUST_TARGET/release/libjarvis_ear_ffi.so"
rm -rf "$app/app/build/generated/uniffi"
cargo run -q -p jarvis-ear-ffi --features bindgen --bin uniffi-bindgen -- \
    generate "$lib" --language kotlin \
    --out-dir "$app/app/build/generated/uniffi" --no-format

# 6. APK (et tests Kotlin). AGP dépouille la bibliothèque à l'empaquetage.
echo "==> APK (JDK : $JAVA_HOME)"
printf 'sdk.dir=%s\n' "$ANDROID_HOME" >"$app/local.properties"
(
    cd "$app"
    # shellcheck disable=SC2086 # options vides à omettre
    ./gradlew --console=plain $offline assembleDebug $test_task
)

# 7. Contenu de l'APK : nos deux bibliothèques, pour arm64-v8a seulement.
apk="$app/app/build/outputs/apk/debug/app-debug.apk"
[[ -f "$apk" ]] || fail "APK absent : $apk"
libs="$(unzip -Z1 "$apk" | grep '^lib/' || true)"
for wanted in "lib/$ABI/libjarvis_ear_ffi.so" "lib/$ABI/libjnidispatch.so"; do
    grep -qx "$wanted" <<<"$libs" || fail "$wanted absente de l'APK"
done
others="$(grep -v "^lib/$ABI/" <<<"$libs" || true)"
[[ -z "$others" ]] || fail "bibliothèques d'une autre ABI dans l'APK : $others"

so_size="$(unzip -l "$apk" "lib/$ABI/libjarvis_ear_ffi.so" | awk 'NR == 4 { print $1 }')"
echo "==> $apk"
echo "    APK : $(du -h "$apk" | cut -f1) ($(stat -c %s "$apk") octets)"
echo "    libjarvis_ear_ffi.so dépouillée : $((so_size / 1024)) Kio"

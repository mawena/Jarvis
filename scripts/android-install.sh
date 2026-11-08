#!/usr/bin/env bash
# Installe l'appli sur le téléphone branché (débogage USB) et la lance.
#
#   scripts/android-install.sh [--no-grant] [--copy-wakeword] [--logs]
#
#   --no-grant        n'accorde aucune permission (ni micro, ni superposition, ni batterie) :
#                     aucun micro ne peut s'ouvrir (essai de fumée)
#   --copy-wakeword   copie le mot d'éveil du PC (~/.local/share/jarvis/wakeword/jarvis.rpw)
#                     sur le téléphone (repli si l'enregistrement sur le téléphone déçoit)
#   --logs            suit le journal : adb logcat -s Jarvis JarvisRust AndroidRuntime
set -euo pipefail

PACKAGE="fr.mawena.jarvis"

usage() {
    sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
}

grant=1
copy_wakeword=0
logs=0
for arg in "$@"; do
    case "$arg" in
        --no-grant) grant=0 ;;
        --copy-wakeword) copy_wakeword=1 ;;
        --logs) logs=1 ;;
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
apk="$repo/apps/android/app/build/outputs/apk/debug/app-debug.apk"
[[ -f "$apk" ]] || fail "APK absent : lancez d'abord scripts/android-build.sh"
command -v adb >/dev/null || fail "adb absent (platform-tools du SDK Android)"

# Exactement un appareil autorisé.
devices="$(adb devices | awk 'NR > 1 && $2 == "device" { print $1 }')"
count="$(grep -c . <<<"$devices" || true)"
if [[ "$count" -ne 1 ]]; then
    unauthorized="$(adb devices | awk 'NR > 1 && $2 == "unauthorized" { print $1 }')"
    if [[ -n "$unauthorized" ]]; then
        fail "téléphone non autorisé : acceptez la clé sur son écran (« Autoriser le débogage USB »)"
    fi
    if [[ "$count" -eq 0 ]]; then
        fail "aucun téléphone : activez les options pour les développeurs (Réglages > À propos > touchez 7 fois « Numéro de build »), puis « Débogage USB », et branchez-le"
    fi
    fail "plusieurs appareils branchés : n'en laissez qu'un"
fi

if [[ "$grant" -eq 1 ]]; then
    # -g accorde micro, notifications et caméra.
    adb install -r -g "$apk"
    # Démarrage seul après un redémarrage (superposition) et écoute écran éteint (batterie).
    adb shell appops set "$PACKAGE" SYSTEM_ALERT_WINDOW allow
    adb shell dumpsys deviceidle whitelist "+$PACKAGE"
else
    adb install -r "$apk"
fi

if [[ "$copy_wakeword" -eq 1 ]]; then
    model="$HOME/.local/share/jarvis/wakeword/jarvis.rpw"
    [[ -f "$model" ]] || fail "mot d'éveil du PC introuvable : $model"
    adb push "$model" /data/local/tmp/jarvis.rpw
    adb shell run-as "$PACKAGE" sh -c 'mkdir -p files/wakeword && cp /data/local/tmp/jarvis.rpw files/wakeword/ && chmod 600 files/wakeword/jarvis.rpw'
    adb shell rm -f /data/local/tmp/jarvis.rpw
    echo "mot d'éveil du PC copié sur le téléphone"
fi

adb shell am start -n "$PACKAGE/.MainActivity"

if [[ "$logs" -eq 1 ]]; then
    exec adb logcat -v time -s Jarvis:V JarvisRust:V AndroidRuntime:E
fi

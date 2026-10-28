#!/usr/bin/env bash
# Télécharge les modèles de Jarvis dans ~/.local/share/jarvis/models/ :
#   - Whisper (ggml, quantifiés q5_1) pour la reconnaissance vocale ;
#   - CAM++ (ONNX, sherpa-onnx) pour l'empreinte vocale.
# Usage : scripts/download-models.sh [small|base|speaker|all]   (défaut : small)
# Idempotent : un fichier déjà complet (taille exacte) n'est pas retéléchargé.
set -euo pipefail

MODELS_DIR="${JARVIS_MODELS_DIR:-$HOME/.local/share/jarvis/models}"
WHISPER_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"
# « recongition » : faute de frappe d'origine dans le nom de la release sherpa-onnx.
SPEAKER_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models"

# fetch <fichier> <taille attendue en octets> <url de base>
fetch() {
    local file="$1" size="$2" base="$3"
    local dest="$MODELS_DIR/$file"
    mkdir -p "$MODELS_DIR"
    if [[ -f "$dest" && "$(stat -c %s "$dest")" == "$size" ]]; then
        echo "déjà présent : $dest"
        return 0
    fi
    echo "téléchargement : $file"
    curl -L --fail --progress-bar -o "$dest.part" "$base/$file"
    local got
    got="$(stat -c %s "$dest.part")"
    if [[ "$got" != "$size" ]]; then
        rm -f "$dest.part"
        echo "erreur : $file fait $got octets, $size attendus" >&2
        return 1
    fi
    mv "$dest.part" "$dest"
    echo "ok : $dest"
}

whisper_small() { fetch ggml-small-q5_1.bin 190085487 "$WHISPER_URL"; }
whisper_base() { fetch ggml-base-q5_1.bin 59707625 "$WHISPER_URL"; }
speaker() { fetch 3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx 29596978 "$SPEAKER_URL"; }

case "${1:-small}" in
    small) whisper_small ;;
    base) whisper_base ;;
    speaker) speaker ;;
    all)
        whisper_small
        whisper_base
        speaker
        ;;
    *)
        echo "usage : $0 [small|base|speaker|all]" >&2
        exit 2
        ;;
esac

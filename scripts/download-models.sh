#!/usr/bin/env bash
# Télécharge les modèles Whisper (ggml, quantifiés q5_1) dans ~/.local/share/jarvis/models/.
# Usage : scripts/download-models.sh [small|base|all]   (défaut : small)
# Idempotent : un fichier déjà complet (taille exacte) n'est pas retéléchargé.
set -euo pipefail

MODELS_DIR="${JARVIS_MODELS_DIR:-$HOME/.local/share/jarvis/models}"
WHISPER_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"

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

case "${1:-small}" in
    small) whisper_small ;;
    base) whisper_base ;;
    all)
        whisper_small
        whisper_base
        ;;
    *)
        echo "usage : $0 [small|base|all]" >&2
        exit 2
        ;;
esac

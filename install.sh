#!/usr/bin/env bash
set -e
cd "$(dirname "$0")"

echo "==> franime_dl - installation (Linux/macOS)"

# --- Rust / cargo ---
if ! command -v cargo >/dev/null 2>&1; then
  echo "==> Rust manquant, installation via rustup..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

# --- ffmpeg ---
if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "==> ffmpeg manquant, installation..."
  if [ "$(uname)" = "Darwin" ]; then
    if command -v brew >/dev/null 2>&1; then
      brew install ffmpeg
    else
      echo "!! Installe Homebrew (https://brew.sh) puis: brew install ffmpeg"
    fi
  elif command -v apt-get >/dev/null 2>&1; then
    sudo apt-get update && sudo apt-get install -y ffmpeg
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y ffmpeg
  elif command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --noconfirm ffmpeg
  elif command -v zypper >/dev/null 2>&1; then
    sudo zypper install -y ffmpeg
  else
    echo "!! Installe ffmpeg manuellement (gestionnaire de paquets non reconnu)."
  fi
fi

# --- Python venv + nodriver + yt-dlp ---
PY=python3
command -v python3 >/dev/null 2>&1 || PY=python
if ! command -v "$PY" >/dev/null 2>&1; then
  echo "!! Python 3 introuvable. Installe-le puis relance ce script."
  exit 1
fi

echo "==> Environnement Python (.venv)..."
"$PY" -m venv .venv
./.venv/bin/pip install --upgrade pip >/dev/null
./.venv/bin/pip install -r python/requirements.txt yt-dlp

# --- Build ---
echo "==> Compilation (cargo build --release)... (ça peut prendre quelques minutes)"
cargo build --release

echo ""
echo "==> Terminé."
echo "    Lancer :  ./target/release/franime_dl"
echo "    Note   :  Chrome ou Chromium doit être installé (sidecar Cloudflare)."

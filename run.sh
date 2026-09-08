#!/usr/bin/env bash
# Launch the scope from a POSIX shell (Git Bash, WSL, Linux).
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$PATH:$HOME/.cargo/bin"

vector="${1:-tests/vectors/bell103-300.wav}"
if [ ! -f "$vector" ] && [ -f "tests/vectors/$vector.wav" ]; then
  vector="tests/vectors/$vector.wav"
fi
if [ ! -f "$vector" ]; then
  echo "no such vector: $vector" >&2
  ls tests/vectors/*.wav 2>/dev/null >&2 || true
  exit 1
fi

cargo build -p gui --release
exec ./target/release/binmodem "$vector"

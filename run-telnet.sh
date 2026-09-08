#!/usr/bin/env bash
# The terminal on its own: a socket to a bulletin board, no modem and no line.
#
# "The board looked wrong" has two causes over a call -- a byte the line
# dropped, or an escape sequence the terminal does not implement. This removes
# the first, so whatever still looks wrong is ours.
#
#   ./run-telnet.sh
#   ./run-telnet.sh vert.synchro.net
set -euo pipefail
cd "$(dirname "$0")"
cargo build -p gui --release
exec ./target/release/binmodem --telnet "$@"

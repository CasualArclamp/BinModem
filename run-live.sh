#!/usr/bin/env bash
# Put a modem of your own on a real line, from a POSIX shell.
#
# The counterpart of run.sh, which replays a capture. This dials.
#
#   ./run-live.sh                          pick devices from a menu
#   ./run-live.sh V32                      and choose the modulation
#
# It offers to start a second modem on the same line, because a modem with
# nothing to dial is not much of a demonstration. One cable is right for that:
# what comes back from a cable is what was written to it, summed with whatever
# else is writing, which is a two-wire pair with two modems across it.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$PATH:$HOME/.cargo/bin"

carrier="${1:-V22B}"
case "$carrier" in
  B103|V22B|V32) ;;
  *) echo "carrier must be B103, V22B or V32, not $carrier" >&2; exit 1 ;;
esac

cargo build -p gui -p modem --release
scope=./target/release/binmodem
answer=./target/release/modem-answer

# Ask the binary which devices there are rather than keeping a second list
# here: it is the thing that has to open them, so its names are the ones that
# matter.
listing=$("$scope" --devices)

# Offer a menu, defaulting to whatever looks like a virtual cable.
choose() {
  local what="$1" prefer="$2" section="$3"
  local -a names=()
  local in_section=0 line
  while IFS= read -r line; do
    case "$line" in
      "input devices"*)  in_section=$([ "$section" = in ] && echo 1 || echo 0); continue ;;
      "output devices"*) in_section=$([ "$section" = out ] && echo 1 || echo 0); continue ;;
    esac
    line="${line#"${line%%[![:space:]]*}"}"
    [ -z "$line" ] && continue
    [ "$in_section" = 1 ] && names+=("$line")
  done <<< "$listing"

  if [ ${#names[@]} -eq 0 ]; then
    echo "no $what devices found" >&2
    exit 1
  fi

  local default=0 i
  for i in "${!names[@]}"; do
    case "${names[$i]}" in *"$prefer"*) default=$i; break ;; esac
  done

  echo >&2
  echo "  which $what?" >&2
  echo >&2
  for i in "${!names[@]}"; do
    local marker=" "
    [ "$i" = "$default" ] && marker="*"
    printf '   %s%d) %s\n' "$marker" "$((i + 1))" "${names[$i]}" >&2
  done
  echo >&2
  read -r -p "  number, or Enter for $((default + 1)): " reply
  local index
  if [ -z "$reply" ]; then
    index=$default
  else
    index=$((reply - 1))
    if [ "$index" -lt 0 ] || [ "$index" -ge ${#names[@]} ]; then
      echo "not a choice: $reply" >&2
      exit 1
    fi
  fi
  printf '%s' "${names[$index]}"
}

in_name=$(choose "input (what the line says)" "CABLE Output" in)
out_name=$(choose "output (what the modem says)" "CABLE Input" out)

echo
read -r -p "  start a board on the same line to dial? [Y/n] " board
if [ "$board" != "n" ] && [ "$board" != "N" ]; then
  "$answer" --in "$in_name" --out "$out_name" --carrier "$carrier" &
  board_pid=$!
  trap 'kill "$board_pid" 2>/dev/null || true' EXIT
  # Let it get its streams open before the caller starts listening.
  sleep 1
fi

echo
echo "  in the terminal pane: AT+MS=$carrier then ATD5551234"
echo "  +++ escapes to command state, ATH hangs up."
echo
"$scope" --live --in "$in_name" --out "$out_name"

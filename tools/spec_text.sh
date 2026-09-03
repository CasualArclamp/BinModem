#!/usr/bin/env bash
# Extract the vendored ITU-T PDFs to greppable text.
# Layout mode is preserved so tables (constellations, S-parameter lists,
# scrambler taps) survive with their columns intact.
set -u
SRC="${1:-docs/specs}"
OUT="${2:-docs/specs/text}"
mkdir -p "$OUT"
n=0
for pdf in "$SRC"/*.pdf; do
  [ -e "$pdf" ] || continue
  base=$(basename "$pdf" .pdf)
  if pdftotext -layout -nopgbrk "$pdf" "$OUT/$base.txt" 2>/dev/null; then
    printf '  %-32s %7s lines\n' "$base.txt" "$(wc -l < "$OUT/$base.txt")"
    n=$((n+1))
  else
    echo "  !! failed: $base"
  fi
done
echo "extracted $n documents -> $OUT"

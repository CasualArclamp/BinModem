#!/usr/bin/env bash
# Fetch the in-force edition of each ITU-T Recommendation we implement against.
# ITU serves the PDF only with a session cookie from the rec landing page.
set -u
OUT="${1:-docs/specs}"
JAR="$(mktemp)"
UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64)"
mkdir -p "$OUT"

RECS="
V.8 V.8bis
V.21 V.22 V.22bis V.23
V.26bis V.26ter V.27ter V.29
V.32 V.32bis V.33 V.17
V.34
V.90 V.92
V.42 V.42bis V.44 V.14
V.24 V.25 V.25bis V.250
V.2 V.56bis
"

fetch_one() {
  local rec="$1" page ed url code
  page="https://www.itu.int/rec/T-REC-${rec}/en"
  curl -sSL -c "$JAR" -b "$JAR" -A "$UA" "$page" -o "$JAR.html" || { echo "  !! landing fetch failed"; return 1; }
  # Prefer the in-force (-I) edition; fall back to the newest superseded (-S).
  ed=$(grep -o "parent=T-REC-${rec}-[0-9]\{6\}-I" "$JAR.html" | head -1 | sed 's/.*parent=//')
  [ -z "$ed" ] && ed=$(grep -o "parent=T-REC-${rec}-[0-9]\{6\}-S" "$JAR.html" | sort -u | tail -1 | sed 's/.*parent=//')
  if [ -z "$ed" ]; then echo "  !! no edition found for $rec"; return 1; fi
  url="https://www.itu.int/rec/dologin_pub.asp?lang=e&id=${ed}!!PDF-E&type=items"
  code=$(curl -sSL -b "$JAR" -c "$JAR" -A "$UA" -e "$page" -o "$OUT/${ed}.pdf" -w '%{http_code}' "$url")
  if [ "$code" = "200" ] && head -c 4 "$OUT/${ed}.pdf" | grep -q '%PDF'; then
    printf '  ok  %-28s %8s bytes\n' "${ed}.pdf" "$(wc -c < "$OUT/${ed}.pdf")"
  else
    echo "  !! $rec download failed (HTTP $code)"; rm -f "$OUT/${ed}.pdf"; return 1
  fi
}

for rec in $RECS; do
  echo "== $rec"
  fetch_one "$rec" || true
done
rm -f "$JAR" "$JAR.html"
echo "done -> $OUT"

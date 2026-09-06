#!/usr/bin/env bash
# Fetch the ITU-T Recommendations we implement against.
# ITU serves the PDF only with a session cookie from the rec landing page.
#
# Usually the in-force edition, but not always: a Recommendation can be revised
# by having something taken out of it, and then the current text is not the one
# to implement against. Naming an edition explicitly, as "V.42@200011", fetches
# that one instead.
set -u
OUT="${1:-docs/specs}"
JAR="$(mktemp)"
UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64)"
mkdir -p "$OUT"

# Editions wanted for a reason, with the reason.
#
#   V.42@200011  The last edition carrying Annex A, the alternative error
#                control procedure -- which is MNP, and is what everything
#                built before LAPM speaks. The 2002 revision deleted it and
#                left the heading behind: "Note that Annex A and Appendix V
#                were deleted from ITU-T Rec. V.42 in the 2002 revision." The
#                deletion says nothing about the modems still using it.
RECS="
V.8 V.8bis
V.21 V.22 V.22bis V.23
V.26bis V.26ter V.27ter V.29
V.32 V.32bis V.33 V.17
V.34
V.90 V.92
V.42 V.42bis V.44 V.14
V.42@200011
V.24 V.25 V.25bis V.250
V.2 V.56bis
"

fetch_one() {
  local spec="$1" rec want page ed url code
  # "V.42@200011" asks for one edition; a bare name takes whichever is current.
  rec="${spec%%@*}"
  want=""
  [ "$spec" != "$rec" ] && want="${spec#*@}"
  page="https://www.itu.int/rec/T-REC-${rec}/en"
  curl -sSL -c "$JAR" -b "$JAR" -A "$UA" "$page" -o "$JAR.html" || { echo "  !! landing fetch failed"; return 1; }
  if [ -n "$want" ]; then
    # The wanted edition, in force or superseded -- which it is depends on when
    # the fetch happens, not on what is wanted.
    ed=$(grep -o "parent=T-REC-${rec}-${want}-[IS]" "$JAR.html" | head -1 | sed 's/.*parent=//')
  else
    # Prefer the in-force (-I) edition; fall back to the newest superseded (-S).
    ed=$(grep -o "parent=T-REC-${rec}-[0-9]\{6\}-I" "$JAR.html" | head -1 | sed 's/.*parent=//')
    [ -z "$ed" ] && ed=$(grep -o "parent=T-REC-${rec}-[0-9]\{6\}-S" "$JAR.html" | sort -u | tail -1 | sed 's/.*parent=//')
  fi
  if [ -z "$ed" ]; then echo "  !! no edition found for $spec"; return 1; fi
  url="https://www.itu.int/rec/dologin_pub.asp?lang=e&id=${ed}!!PDF-E&type=items"
  code=$(curl -sSL -b "$JAR" -c "$JAR" -A "$UA" -e "$page" -o "$OUT/${ed}.pdf" -w '%{http_code}' "$url")
  if [ "$code" = "200" ] && head -c 4 "$OUT/${ed}.pdf" | grep -q '%PDF'; then
    printf '  ok  %-28s %8s bytes\n' "${ed}.pdf" "$(wc -c < "$OUT/${ed}.pdf")"
  else
    echo "  !! $rec download failed (HTTP $code)"; rm -f "$OUT/${ed}.pdf"; return 1
  fi
}

for rec in $RECS; do
  case "$rec" in \#*) continue ;; esac
  echo "== $rec"
  fetch_one "$rec" || true
done
rm -f "$JAR" "$JAR.html"
echo "done -> $OUT"

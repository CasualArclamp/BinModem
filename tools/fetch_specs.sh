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

# T.30 and T.4 are not modulations. They are what a fax call does over one --
# the control procedure and the page coding -- and they are here because a
# number that answers with 2100 Hz and then V.21 is a fax machine, and the only
# way to know what it will accept is to read its DIS.

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
T.30 T.4
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

# The RFCs, for what goes over the call. Text rather than PDF, so they go
# straight to the same place the extracted Recommendations do and are greppable
# on arrival.
#
# The link: 1661 is PPP itself and 1662 the framing under it; 1334 and 1994 are
# the two ways a far end asks who is calling; 1332 is how an address is agreed;
# 1144 is Van Jacobson header compression, which is what made dial-up bearable.
#
# What crosses it: 791 the datagram, 792 the echo, 1071 the checksum over both,
# and 9293 -- TCP, which obsoletes 793 and gathers fifty years of amendments to
# it into one document, so that is the one to implement against. 1122 says what
# a host must do with all of them, 6298 how to time a retransmission, 5681 how
# fast to send, and 7323 how to say a window larger than sixteen bits.
# Beside the extracted Recommendations, which is where spec_text.sh puts theirs
# and where anything reading them expects to look.
TEXT="${2:-$OUT/text}"
mkdir -p "$TEXT"
RFCS="1661 1662 1332 1334 1994 1144 791 792 1071 9293 1122 6298 5681 7323"
n=0
for rfc in $RFCS; do
  if curl -sS --fail --max-time 30 -o "$TEXT/rfc$rfc.txt" \
      "https://www.rfc-editor.org/rfc/rfc$rfc.txt"; then
    printf '  %-32s %7s lines\n' "rfc$rfc.txt" "$(wc -l < "$TEXT/rfc$rfc.txt")"
    n=$((n+1))
  else
    echo "  !! failed: rfc$rfc"
  fi
done
echo "fetched $n RFCs -> $TEXT"

#!/usr/bin/env bash
# Diagnostic for #706: run every fault-loop scenario on a macOS host and print
# one VERDICT line per scenario (also appended to $GITHUB_STEP_SUMMARY).
#
# Usage: run-macos.sh <release-binary> <different-binary-for-in-place-rewrite>
#
# Each scenario starts one fault-loop-706 process. The script waits for the
# process to die or fault. If the process is still alive after the fault, the
# script records its CPU time over 3 s and a `sample` of its stacks, then
# SIGKILLs it. These are throwaway test processes that hold no data.
set -uo pipefail

BIN_SRC=$1
ALT_SRC=$2
OUT=${OUT:-$PWD/fl706-out}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/fl706.XXXXXX")
MNT=$WORK/mnt
mkdir -p "$OUT" "$MNT"
SUMMARY=${GITHUB_STEP_SUMMARY:-/dev/null}

sw_vers || true
uname -a
echo "| scenario | outcome | detail |" >>"$SUMMARY"
echo "|---|---|---|" >>"$SUMMARY"

signame() {
  local s=$(( $1 - 128 ))
  case $s in 4) echo SIGILL ;; 5) echo SIGTRAP ;; 6) echo SIGABRT ;; 9) echo SIGKILL ;; 10) echo SIGBUS ;; 11) echo SIGSEGV ;; *) echo "sig$s" ;; esac
}

attach_image() {
  rm -f "$WORK/fl706.dmg"
  hdiutil create -quiet -size 64m -fs HFS+ -volname FL706 "$WORK/fl706.dmg" || return 1
  hdiutil attach -quiet -nobrowse -noverify -mountpoint "$MNT" "$WORK/fl706.dmg" || return 1
}

detach_image() {
  hdiutil detach -force "$MNT" >"$OUT/$1.detach.txt" 2>&1
  echo "detach rc=$?" >>"$OUT/$1.detach.txt"
}

# run_case NAME DIR MODE ACTION [SIGN]
#   DIR    where the binary is copied and run from
#   MODE   fault-loop-706 mode
#   ACTION none | detach | rewrite
#   SIGN   keep | runtime (ad-hoc re-sign with the hardened-runtime flag)
run_case() {
  local name=$1 dir=$2 mode=$3 action=$4 sign=${5:-keep}
  local exe=$dir/fl706 trig=$WORK/$name.trigger log=$OUT/$name.log
  rm -f "$trig" "$trig.res"
  cp "$BIN_SRC" "$exe"
  if [ "$sign" = runtime ]; then
    codesign -s - -f -o runtime "$exe" >"$OUT/$name.codesign.txt" 2>&1
  fi
  codesign -dv "$exe" >>"$OUT/$name.codesign.txt" 2>&1
  local ino_before
  ino_before=$(stat -f %i "$exe")
  "$exe" "$mode" "$trig" >"$log" 2>&1 &
  local pid=$!
  if [ "$mode" != control ] && [ "$mode" != noexec ]; then
    for _ in $(seq 1 100); do grep -q 'state=armed' "$log" && break; kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
    sleep 2
    local res_before
    res_before=$(head -c2 "$trig.res" 2>/dev/null || echo '??')
    case $action in
      detach) detach_image "$name" ;;
      rewrite) cat "$ALT_SRC" >"$exe"; echo "inode before=$ino_before after=$(stat -f %i "$exe" 2>/dev/null)" >>"$log.driver" ;;
    esac
    sleep 1
    local res_after
    res_after=$(head -c2 "$trig.res" 2>/dev/null || echo '??')
    echo "residency(handler,null_write) before_action=$res_before at_trigger=$res_after" >>"$log.driver"
    touch "$trig"
  fi
  local alive=1
  for _ in $(seq 1 80); do
    if ! kill -0 "$pid" 2>/dev/null; then alive=0; break; fi
    sleep 0.1
  done
  local outcome detail
  if [ $alive = 0 ]; then
    wait "$pid"; local st=$?
    if [ $st -gt 128 ]; then outcome="died $(signame $st)"; else outcome="exited $st"; fi
    detail=$(grep -h -E 'survived|mprotect|mlock|evict|residency' "$log" "$log.driver" 2>/dev/null | tr '\n' ' ' | cut -c1-300)
  else
    local t1 t2
    t1=$(ps -o time= -p "$pid" | tr -d ' ')
    sleep 3
    t2=$(ps -o time= -p "$pid" | tr -d ' ')
    sample "$pid" 2 -file "$OUT/$name.sample.txt" >/dev/null 2>&1 || echo "sample failed" >"$OUT/$name.sample.txt"
    local top
    top=$(grep -E 'signal_handler|_sigtramp|null_write' "$OUT/$name.sample.txt" | head -6 | sed -E 's/^[ +!:|]*//' | tr -s ' ' | tr '\n' ';' | cut -c1-400)
    outcome="ALIVE after fault, cpu ${t1} -> ${t2} over 3s"
    detail="stack: ${top:-n/a}; $(grep -h residency "$log.driver" 2>/dev/null)"
    kill -KILL "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
  fi
  echo "VERDICT $name: $outcome | $detail"
  echo "| $name | $outcome | ${detail//|//} |" >>"$SUMMARY"
  echo "----- $name log"; cat "$log" "$log.driver" 2>/dev/null; echo
}

LOCAL=$WORK/local
mkdir -p "$LOCAL"

run_case control "$LOCAL" control none
run_case noexec "$LOCAL" noexec none
run_case armed-local-nothing "$LOCAL" armed none
run_case rewrite-inplace-adhoc "$LOCAL" armed rewrite
run_case rewrite-inplace-runtime "$LOCAL" armed rewrite runtime

for m in armed armed-noevict armed-mlock; do
  if attach_image; then
    run_case "dmg-detach-$m" "$MNT" "$m" detach
    hdiutil detach -force "$MNT" >/dev/null 2>&1 || true
  else
    echo "VERDICT dmg-detach-$m: could not create/attach the disk image"
  fi
done

if attach_image; then
  run_case dmg-detach-armed-runtime "$MNT" armed detach runtime
  hdiutil detach -force "$MNT" >/dev/null 2>&1 || true
fi

echo "done; logs in $OUT"

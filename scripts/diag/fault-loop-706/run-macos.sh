#!/usr/bin/env bash
# Diagnostic for #706: run every fault-loop scenario on a macOS host and print
# one VERDICT line per scenario (also appended to $GITHUB_STEP_SUMMARY).
#
# Usage: run-macos.sh <release-binary> <different-binary-for-in-place-rewrite>
#
# Each scenario starts one fault-loop-706 process. The script waits for the
# process to die or fault. If the process is still alive after the fault, the
# script records its CPU time over 3 s, its PC and stacks (lldb, spindump), then
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

# run_case NAME DIR MODE ACTION [SIGN] [RESETHAND]
#   DIR    where the binary is copied and run from
#   MODE   fault-loop-706 mode
#   ACTION none | detach | rewrite
#   SIGN   keep | runtime (ad-hoc re-sign with the hardened-runtime flag)
run_case() {
  local name=$1 dir=$2 mode=$3 action=$4 sign=${5:-keep} reset=${6:-0}
  local exe=$dir/fl706 trig=$WORK/$name.trigger log=$OUT/$name.log
  rm -f "$trig" "$trig.res"
  # A fresh inode every time: copying over a signed binary in place makes the
  # kernel kill the next exec of it (a cached signature for that vnode).
  rm -f "$exe"
  cp "$BIN_SRC" "$exe"
  if [ "$sign" = runtime ]; then
    codesign -s - -f -o runtime "$exe" >"$OUT/$name.codesign.txt" 2>&1
  fi
  codesign -dv "$exe" >>"$OUT/$name.codesign.txt" 2>&1
  local ino_before
  ino_before=$(stat -f %i "$exe")
  FL706_RESETHAND=$reset "$exe" "$mode" "$trig" >"$log" 2>&1 &
  local pid=$!
  if [ "$mode" != control ] && [ "$mode" != noexec ] && [ "$mode" != overflow ]; then
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
    perl -e "alarm 60; exec @ARGV" sudo -n lldb --batch -p "$pid" -o "register read pc sp" -o "thread backtrace all" -o "process detach" >"$OUT/$name.lldb.txt" 2>&1
    perl -e "alarm 60; exec @ARGV" sudo -n spindump "$pid" 2 10 -file "$OUT/$name.spindump.txt" >/dev/null 2>&1 || echo "spindump failed" >>"$OUT/$name.spindump.txt"
    local pc handler_addr where
    pc=$(grep -m1 -o -E 'pc = 0x[0-9a-f]+' "$OUT/$name.lldb.txt" | awk '{print $3}')
    handler_addr=$(grep -m1 -o -E 'handler=0x[0-9a-f]+' "$log" | cut -d= -f2)
    if [ -n "$pc" ] && [ -n "$handler_addr" ]; then
      where="pc=$pc handler=$handler_addr offset=$(( pc - handler_addr ))"
    else
      where="pc=${pc:-?} handler=${handler_addr:-?}"
    fi
    local top
    top=$(grep -E 'frame #[0-9]+' "$OUT/$name.lldb.txt" | head -4 | sed -E 's/^[ *]*//' | tr -s ' ' | tr '\n' ';' | cut -c1-400)
    outcome="ALIVE after fault, cpu ${t1} -> ${t2} over 3s"
    detail="$where; frames: ${top:-n/a}; $(grep -h residency "$log.driver" 2>/dev/null)"
    kill -KILL "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
  fi
  echo "VERDICT $name: $outcome | $detail"
  echo "| $name | $outcome | ${detail//|//} |" >>"$SUMMARY"
  echo "----- $name log"; cat "$log" "$log.driver" 2>/dev/null; echo
}

LOCAL=$WORK/local
mkdir -p "$LOCAL"

for r in 0 1; do
  run_case "control-r$r" "$LOCAL" control none keep $r
  run_case "overflow-r$r" "$LOCAL" overflow none keep $r
  run_case "noexec-r$r" "$LOCAL" noexec none keep $r
  run_case "noexec-runtime-r$r" "$LOCAL" noexec none runtime $r
done
run_case rewrite-inplace-adhoc "$LOCAL" armed rewrite

for r in 0 1; do
  for m in armed armed-noevict; do
    if attach_image; then
      run_case "dmg-detach-$m-r$r" "$MNT" "$m" detach keep $r
      hdiutil detach -force "$MNT" >/dev/null 2>&1 || true
    else
      echo "VERDICT dmg-detach-$m-r$r: could not create/attach the disk image"
    fi
  done
  if attach_image; then
    run_case "dmg-detach-armed-runtime-r$r" "$MNT" armed detach runtime $r
    hdiutil detach -force "$MNT" >/dev/null 2>&1 || true
  fi
done

echo "done; logs in $OUT"

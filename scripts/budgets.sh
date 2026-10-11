#!/bin/zsh
# Budgets from SPEC.md § Goals: binary size, idle memory with 15 tabs, CLI latency, and
# responsiveness while another tab floods output. Builds release binaries and runs a private
# server under a temporary CTX_HOME. Prints measurements; exits non-zero if a budget is missed.
set -eu
zmodload zsh/datetime
cd "${0:A:h}/.."
cargo build --release --quiet
CTX=$PWD/target/release/ctx
export CTX_HOME=$(mktemp -d /tmp/ctx-budgets.XXXXXX)
export CTX_ASSIST=off
fail=0
check() { # name value limit unit
  if (( $2 <= $3 )); then print "ok    $1: $2 $4 (budget $3)"; else print "OVER  $1: $2 $4 (budget $3)"; fail=1; fi
}

size_ctx=$(( $(stat -f %z target/release/ctx) / 1024 ))
size_launch=$(( $(stat -f %z target/release/ctx-launch) / 1024 ))
check "ctx binary" $size_ctx 10240 KB
check "ctx-launch binary" $size_launch 1024 KB

$CTX server >"$CTX_HOME/server.log" 2>&1 &
server=$!
trap 'kill $server 2>/dev/null; rm -rf "$CTX_HOME"' EXIT
for i in {1..50}; do $CTX status >/dev/null 2>&1 && break; sleep 0.1; done

# 15 tabs, each with a screenful of output and a filled hot region (2,000 lines), then idle.
for i in {1..15}; do $CTX open tab named "Budget $i" at /tmp >/dev/null; done
for i in {1..15}; do $CTX run in "Budget $i" "seq 1 2000; ls -la /usr/bin | head -40" >/dev/null 2>&1; done
sleep 5
mem=$(( $($CTX status as json | python3 -c 'import json,sys; print(json.load(sys.stdin)["memory_bytes"])') / 1024 / 1024 ))
check "server memory, 15 tabs" $mem 40 MB

# CLI round trip (median of 21 calls).
times=()
for i in {1..21}; do
  t0=$EPOCHREALTIME
  $CTX list tabs >/dev/null
  t1=$EPOCHREALTIME
  times+=$(( (t1 - t0) * 1000 ))
done
median=${${(on)times}[11]}
check "ctx list tabs (median)" ${median%.*} 10 ms

# Responsiveness under load: a tab printing as fast as it can while another runs commands.
$CTX run in "Budget 1" "yes" without waiting >/dev/null
sleep 1
times=()
for i in {1..11}; do
  t0=$EPOCHREALTIME
  $CTX run in "Budget 2" "true" >/dev/null
  t1=$EPOCHREALTIME
  times+=$(( (t1 - t0) * 1000 ))
done
median=${${(on)times}[6]}
check "run \"true\" while another tab floods (median)" ${median%.*} 150 ms
$CTX press ctrl-c in "Budget 1" >/dev/null
mem=$(( $($CTX status as json | python3 -c 'import json,sys; print(json.load(sys.stdin)["memory_bytes"])') / 1024 / 1024 ))
check "server memory after the flood" $mem 240 MB
exit $fail

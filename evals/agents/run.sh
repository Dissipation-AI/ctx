#!/bin/zsh
# Agent evaluation (SPEC.md, "Evaluation"): runs each task with each configuration and records
# task success, wall-clock time, tokens, tool calls and ctx misuse.
#
#   evals/agents/run.sh [--configs baseline,instructions,mcp,mcp-json,mcp-toon,hook]
#                       [--tasks fix-failing-test,...] [--repeat N] [--model MODEL]
#
# Needs `claude` (Claude Code) and a built ctx (`cargo build --release`). Each run gets a fresh
# folder and its own ctx server (CTX_HOME), so runs don't see each other.
set -u
zmodload zsh/datetime
here=${0:A:h}
root=${here:h:h}
configs=(baseline instructions mcp hook)
tasks=(${(f)"$(ls $here/tasks)"})
repeat=1
model=""
while (( $# )); do
  case $1 in
    --configs) configs=(${(s:,:)2}); shift 2 ;;
    --tasks) tasks=(${(s:,:)2}); shift 2 ;;
    --repeat) repeat=$2; shift 2 ;;
    --model) model=$2; shift 2 ;;
    *) print -u2 "unknown option $1"; exit 2 ;;
  esac
done
ctx_bin=$root/target/release/ctx
[[ -x $ctx_bin ]] || { print -u2 "build ctx first: cargo build --release"; exit 2; }
out=$here/results/$(date +%Y-%m-%d-%H%M%S)
mkdir -p $out
print "config,task,run,success,seconds,input_tokens,output_tokens,turns,cost_usd,ctx_calls,ctx_unparsed,ctx_unbounded" > $out/results.csv

for cfg in $configs; do for task in $tasks; do for n in $(seq 1 $repeat); do
  work=$(mktemp -d /tmp/ctx-eval.XXXXXX)
  export CTX_HOME=$work/.ctx CTX_ASSIST=off
  mkdir -p $work/bin $CTX_HOME
  # A ctx on PATH that records each call and its exit status, then runs the real one.
  cat > $work/bin/ctx <<SHIM
#!/bin/sh
"$ctx_bin" "\$@" 2>"$work/.last-err"; st=\$?
cat "$work/.last-err" >&2
printf '%s\t%s\t%s\n' "\$st" "\$*" "\$(head -c 200 "$work/.last-err" | tr '\n' ' ')" >> "$work/.ctx-calls"
exit \$st
SHIM
  chmod +x $work/bin/ctx
  export PATH=$work/bin:$PATH
  "$ctx_bin" server >$CTX_HOME/server.log 2>&1 &
  server=$!
  for i in {1..50}; do ctx status >/dev/null 2>&1 && break; sleep 0.1; done
  (cd $work && sh $here/tasks/$task/setup.sh >/dev/null 2>&1)
  : > $work/.ctx-calls
  args=(-p "$(cat $here/tasks/$task/task.md)" --output-format json --permission-mode bypassPermissions)
  [[ -n $model ]] && args+=(--model $model)
  case $cfg in
    baseline) ;;
    instructions) : > $work/CLAUDE.md; (cd $work && ctx install instructions for agents at $work >/dev/null) ;;
    mcp|mcp-json|mcp-toon)
      : > $work/CLAUDE.md; (cd $work && ctx install instructions for agents at $work >/dev/null)
      args+=(--mcp-config $root/integrations/claude/.mcp.json --disallowedTools Bash)
      [[ $cfg == mcp-json ]] && args+=(--append-system-prompt "Add as json to ctx sentences when you need structured answers.")
      [[ $cfg == mcp-toon ]] && args+=(--append-system-prompt "Add as toon to ctx sentences when you need structured answers.") ;;
    hook) : > $work/CLAUDE.md; (cd $work && ctx install instructions for agents at $work >/dev/null); args+=(--plugin-dir $root/integrations/claude) ;;
  esac
  t0=$EPOCHREALTIME
  (cd $work && claude "${args[@]}" > $work/.claude.json 2>$work/.claude.err)
  t1=$EPOCHREALTIME
  (cd $work && sh $here/tasks/$task/check.sh >/dev/null 2>&1) && ok=1 || ok=0
  read in_tok out_tok turns cost <<<"$(python3 - "$work/.claude.json" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1]))
    u = d.get("usage", {})
    tin = u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0) + u.get("cache_creation_input_tokens", 0)
    print(tin, u.get("output_tokens", 0), d.get("num_turns", 0), d.get("total_cost_usd", 0))
except Exception:
    print(0, 0, 0, 0)
PY
)"
  calls=$(wc -l < $work/.ctx-calls | tr -d ' ')
  unparsed=$(grep -c -E "isn't a ctx verb|Did you mean|Put it in quotes|isn't a valid ctx command" $work/.ctx-calls)
  unbounded=$(grep -c -P "\tread everything" $work/.ctx-calls 2>/dev/null || grep -c "read everything" $work/.ctx-calls)
  secs=$(( t1 - t0 ))
  print "$cfg,$task,$n,$ok,${secs%.*},$in_tok,$out_tok,$turns,$cost,$calls,$unparsed,$unbounded" >> $out/results.csv
  print "$cfg $task #$n: success=$ok ${secs%.*}s tokens=$in_tok/$out_tok turns=$turns ctx_calls=$calls unparsed=$unparsed"
  kill $server 2>/dev/null; wait $server 2>/dev/null
  pkill -f "$work" 2>/dev/null
  cp $work/.claude.json $out/$cfg-$task-$n.json 2>/dev/null
  rm -rf $work
done; done; done
python3 $here/summarize.py $out/results.csv | tee $out/summary.md

"""Summarise evals/agents results against SPEC.md's bar: a configuration may become a default only
if it's no worse on task success and no more than ~10% worse on time and tokens than the baseline."""
import csv, statistics, sys
from collections import defaultdict

rows = list(csv.DictReader(open(sys.argv[1])))
by = defaultdict(list)
for r in rows:
    by[r["config"]].append(r)

def med(vals):
    return statistics.median(vals) if vals else 0

stats = {}
for cfg, rs in by.items():
    stats[cfg] = {
        "runs": len(rs),
        "success": sum(int(r["success"]) for r in rs) / len(rs),
        "seconds": med([float(r["seconds"]) for r in rs]),
        "tokens": med([int(r["input_tokens"]) + int(r["output_tokens"]) for r in rs]),
        "turns": med([int(r["turns"]) for r in rs]),
        "unparsed": sum(int(r["ctx_unparsed"]) for r in rs),
        "unbounded": sum(int(r["ctx_unbounded"]) for r in rs),
        "calls": sum(int(r["ctx_calls"]) for r in rs),
    }

print("| config | runs | success | median seconds | median tokens | median turns | ctx calls | unparsed | unbounded reads | meets the bar |")
print("|---|---|---|---|---|---|---|---|---|---|")
base = stats.get("baseline")
for cfg, s in stats.items():
    verdict = "(baseline)"
    if cfg != "baseline" and base:
        ok = s["success"] >= base["success"] and s["seconds"] <= base["seconds"] * 1.1 and s["tokens"] <= base["tokens"] * 1.1
        verdict = "yes" if ok else "no"
    elif not base:
        verdict = "no baseline"
    print(f"| {cfg} | {s['runs']} | {s['success']:.0%} | {s['seconds']:.0f} | {s['tokens']:.0f} | {s['turns']:.0f} | {s['calls']} | {s['unparsed']} | {s['unbounded']} | {verdict} |")

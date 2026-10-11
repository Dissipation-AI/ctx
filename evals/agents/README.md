# Agent evaluation

SPEC.md, "Evaluation (gates tool replacement)": replacing an agent's shell with ctx can slow it
down, raise token spend or lower quality, so no configuration is on by default until it's measured.

Tasks (`tasks/<name>/`: `setup.sh` builds a fresh folder, `task.md` is the prompt, `check.sh`
decides success):

| Task | What it exercises |
|---|---|
| fix-failing-test | read code, run tests, edit, rerun |
| dev-server | start a long-running server, verify it, leave it running |
| debug-from-logs | find the cause in a long log |
| multi-step-refactor | several edits and checks in sequence |
| pick-up-task | continue work another agent started (in a ctx tab, when ctx is there) |

Configurations: `baseline` (the agent's own shell), `instructions` (+ ctx's instruction block),
`mcp` (ctx's MCP tools, built-in shell off), `mcp-json` / `mcp-toon` (same, asking for structured
answers), `hook` (the Claude Code plugin's hook that routes Bash through `ctx run`).

Measured per run: task success, wall-clock time, input and output tokens, turns, ctx calls, and
misuse (sentences that didn't parse, `read everything`). A configuration meets the bar if it's no
worse on success and no more than ~10% worse on median time and tokens than the baseline.

```sh
cargo build --release
evals/agents/run.sh --configs baseline,instructions,mcp,hook --repeat 3
```

Results land in `results/<date>/` (`results.csv`, `summary.md`, and each run's JSON).

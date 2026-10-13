# ctx for Claude Code

A plugin that makes ctx Claude Code's shell:

- `.mcp.json` registers `ctx mcp`, so Claude can use ctx's verbs as tools.
- `hooks/hooks.json` adds a `PreToolUse` hook on `Bash`: `ctx hook claude` rewrites each command
  into `ctx run in "Claude <session>" '<command>'`, in a tab named for the session (opened in the
  session's folder on first use). Background commands become `… without waiting`; the Bash tool's
  timeout becomes `within`. With `CTX_CLAUDE_HOOK=deny` it refuses Bash calls instead and points
  at the MCP tools.
- The instruction block comes from `ctx install instructions for claude`.

**Not on by default.** SPEC.md, "Evaluation", gates this: it ships only once `evals/agents` shows it
is no worse on task success and no more than ~10% worse on time and tokens than Claude's own
shell. Try it with `claude --plugin-dir ctx/integrations/claude`, or install it with
`ctx install hooks for claude` (`claude plugin install` from `integrations/`, which is a
marketplace; add `at <folder>` for one project, in Claude's local scope).

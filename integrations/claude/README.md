# ctx for Claude Code

A plugin that makes ctx Claude Code's shell:

- `hooks/hooks.json` adds a `PreToolUse` hook on `Bash`: `ctx hook claude` rewrites each command
  into `ctx run in "Claude <session>" '<command>'`, in a tab named for the session (opened in the
  session's folder on first use). Background commands become `… without waiting`; the Bash tool's
  timeout becomes `within`.
- The instruction block comes from `ctx install instructions for claude`.

**Not on by default.** SPEC.md, "Evaluation", gates this: it ships only once `evals/agents` shows it
is no worse on task success and no more than ~10% worse on time and tokens than Claude's own
shell. Try it with `claude --plugin-dir ctx/integrations/claude`, or install it with
`ctx install hooks for claude` (`claude plugin install` from `integrations/`, which is a
marketplace; add `at <folder>` for one project, in Claude's local scope).

**ctx as the only shell, without the hook.** Claude Code can remove its Bash tool entirely: a bare
`"Bash"` in `permissions.deny` takes it out of Claude's context (a plugin can't set this; plugin
settings only take `agent` and `subagentStatusLine`). Then give Claude `ctx mcp`'s `shell` tool
instead:

```sh
claude mcp add --scope user ctx -- ctx mcp
```

```json
{ "permissions": { "deny": ["Bash"], "allow": ["mcp__ctx__shell"] } }
```

in `~/.claude/settings.json` (or a project's `.claude/settings.local.json`). For one run:
`claude --disallowedTools Bash --mcp-config '{"mcpServers":{"ctx":{"command":"ctx","args":["mcp"]}}}'`.
Checked with Claude Code 2.1.310 on 2026-10-23: with Bash denied, `mcp__ctx__shell` is its only
shell, and commands run in a tab named `Claude <pid>`. Use one setup or the other, not both.

# ctx for Claude Code

A plugin that makes ctx Claude Code's shell:

- `bin/ctx-shell` is Claude Code's shell wrapper: `ctx install hooks for claude` names it in
  `env.CLAUDE_CODE_SHELL_PREFIX` of `~/.claude/settings.json` (or the project's
  `.claude/settings.local.json`), and Claude Code then hands it every shell line it runs. For a Bash
  call it runs the command (what Claude wrote, taken out of Claude's `eval '…'` line) as
  `ctx run in "Claude <session>" '<command>'`, in a tab named for the session (opened in the
  session's folder on first use), then tells Claude the folder the tab ended in. Anything else
  (hooks, the status line, MCP servers, a single ctx sentence) runs as it would have.
- `hooks/hooks.json` adds a `PreToolUse` hook on `Bash`, `ctx hook claude`, that only notes each
  call's options for the wrapper: the Bash tool's timeout becomes `within`, and a background
  command runs `… without waiting` in its own copy of the session tab, named for the command
  (`Claude <session> - bun run dev`, reused when idle). A call that finds the session tab busy runs
  in the first idle `Claude <session> 2`, `3`…, so the session's shell is never "busy".
- The hook never changes the call. Auto mode reviews the input Claude wrote and refuses a call a
  hook changed ("a hook changed this call's input after auto mode reviewed it"; checked with Claude
  Code 2.1.323 on 2026-11-21, for plugin and settings hooks alike), so an earlier version that
  rewrote each command into `ctx run` cost a refusal per command and taught Claude to write the
  `ctx run` wrapper by hand, with its quoting. With the wrapper, what auto mode reviews is what runs.
- The instruction block comes from `ctx install instructions for claude`.

**Not on by default.** SPEC.md, "Evaluation", gates this: it ships only once `evals/agents` shows it
is no worse on task success and no more than ~10% worse on time and tokens than Claude's own
shell. Try it with `claude --plugin-dir ctx/integrations/claude`, or install it with
`ctx install hooks for claude` (`claude plugin install` from `integrations/`, which is a
marketplace; add `at <folder>` for one project, in Claude's local scope). Run in a terminal, it
offers ctx's skills (`ctx-skills`) too; `ctx install skills for claude` adds them on their own.

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

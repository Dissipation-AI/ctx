# ctx for the OpenAI Codex CLI

A Codex plugin that makes ctx Codex's shell:

- `hooks/hooks.json` adds a `PreToolUse` hook on `Bash` (which also covers `exec_command`):
  `ctx hook codex` rewrites each command into `ctx run in "Codex <session>" '<command>'`, in a tab
  named for the session (opened in the session's folder on first use; a call's own `workdir` is
  honoured with a `cd`). A single ctx sentence runs as is.
- Codex only takes `updatedInput` alongside `"permissionDecision": "allow"`; that doesn't skip its
  approval or sandbox.
- `../.agents/plugins/marketplace.json` makes `integrations/` a Codex marketplace.

Install it with `ctx install hooks for codex` (`codex plugin marketplace add` +
`codex plugin add ctx@ctx`), or for one project with `ctx install hooks for codex at <folder>`
(an entry in that project's `.codex/hooks.json`). Codex runs a new hook only once you've trusted
it: open `/hooks` and trust ctx's. `ctx remove hooks for codex` undoes either.

**Sandbox.** In Codex's default sandbox `ctx` can't reach its server ("connecting to … was
denied"); Codex then asks to run the command outside the sandbox, and you approve it. The command
runs in the ctx tab either way, so your approval is the check, not the sandbox.

`ctx install instructions for codex` writes the instruction block to `~/.codex/AGENTS.md`;
the hook doesn't need it.

# ctx for Antigravity

A plugin that makes ctx Antigravity's shell:

- `hooks.json` adds a `PreToolUse` hook on `run_command`: `ctx hook antigravity` rewrites each
  command into `ctx run in "Antigravity <conversation>" '<command>'`, in a tab named for the
  conversation (opened in the workspace on first use). When the call's `Cwd` isn't the tab's
  folder, the command changes folder first; non-blocking calls become `… without waiting`. A
  single ctx sentence runs as is; anything chained onto it goes to the tab. With
  `CTX_ANTIGRAVITY_HOOK=deny` it refuses shell calls instead and points at ctx.
- Every reply's `decision` is `ask`: Antigravity denies a call whose hook gives no decision, and
  `allow` would skip your approval, so you're asked as usual (honouring what you've always allowed).
- **Sandbox:** the command runs in the ctx tab, outside Antigravity's sandbox, and inside the
  sandbox `ctx` can't reach its server anyway, so rewritten calls set `BypassSandbox`. With ctx,
  your approval is the check, not the sandbox. Note that every command now starts with `ctx`, so
  "always allow commands that start with 'ctx'" allows *everything*.
- No rules or instruction files: the agent isn't told about ctx, it just finds its shell persists.

Install it with `ctx install hooks for antigravity` (runs `agy plugin install` on a copy of this
folder that names your `ctx`), or for one project with `ctx install hooks for antigravity at
<folder>` (an entry in that project's `.agents/plugins.json`). `ctx remove hooks for antigravity`
undoes either.

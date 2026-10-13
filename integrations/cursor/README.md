# ctx for the Cursor CLI

`hooks.json` is a `preToolUse` hook on Cursor's `Shell` tool: `ctx hook cursor` rewrites each
command into `ctx run in "Cursor <conversation>" '<command>'` (with `updated_input`), in a tab
named for the conversation. The call's `working_directory` is honoured with a `cd`, background
calls become `… without waiting`, and a single ctx sentence runs as is. It never sets
`permission`, so Cursor's own approval still applies.

Cursor's CLI has no plugin installer, so `ctx install hooks for cursor` adds this entry to
`~/.cursor/hooks.json` (or, with `at <folder>`, to that project's `.cursor/hooks.json`), leaving
any other hooks there alone. `ctx remove hooks for cursor` takes it out again. With Cursor's
sandbox on, `ctx` can't reach its server, the same as in Codex and Antigravity.

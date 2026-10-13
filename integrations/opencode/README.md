# ctx for opencode

An opencode plugin: `index.js` handles `tool.execute.before` for `bash`. It hands the command to
`ctx hook opencode`, which rewrites it into `ctx run in "OpenCode <session>" '<command>'` in a tab
named for the session; the plugin then replaces `output.args.command`. The call's `workdir` is
honoured with a `cd`, `background` calls become `… without waiting`, and a `timeout` becomes
`within`. If ctx isn't reachable, the command runs as it would have.

Install it with `ctx install hooks for opencode` (`opencode plugin <path> --global`), or for one
project with `ctx install hooks for opencode at <folder>` (`opencode plugin <path>` there).
`ctx remove hooks for opencode` takes ctx's entry out of opencode's `plugin` list; a config with
comments (`.jsonc`) has to be edited by hand.

# ctx for pi

A pi package: `ctx.ts` is an extension that handles `tool_call` for `bash`. It hands the command
to `ctx hook pi`, which rewrites it into `ctx run in "Pi <session>" '<command>'` in a tab named
for the session; the extension then patches `event.input.command` in place. A bash `timeout`
becomes `within`, slightly shorter. If ctx isn't reachable, the command runs as it would have.

Install it with `ctx install hooks for pi` (`pi install` on a copy of this folder that names your
`ctx`), or for one project with `ctx install hooks for pi at <folder>` (`pi install -l`, recorded
in that project's `.pi/settings.json`). `ctx remove hooks for pi` undoes either.

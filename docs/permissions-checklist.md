# Permissions checklist (manual, each macOS release)

TCC can't be scripted reliably, so these are checked by a person at the machine after each macOS
update and recorded in `docs/phase0.md`. Use a release build installed where launchd runs it
(`ctx list tabs` registers the server; `launchctl bootout gui/$UID/com.ctx.server` removes it).

1. **Responsible process.** In a tab: `ctx run in "<tab>" "sleep 30" without waiting`, then check
   the tab's processes answer to `ctx-launch`
   (`cargo run --release -p ctx-server --example responsible <pids>`). The server answers to
   itself, never to a terminal app.
2. **Full Disk Access fails closed.** In a tab: `ls ~/Library/Mail ~/Library/Messages
   ~/Library/Safari "/Library/Application Support/com.apple.TCC"` → `Operation not permitted`,
   no prompt, nothing new in System Settings.
3. **Files & Folders.** In a tab: `ls ~/Desktop ~/Documents ~/Downloads`. Expected by the spec:
   denied or a prompt that names ctx-launch. Found 2026-10-11: readable without a prompt (see
   phase0.md). Try with a bundle identity embedded in ctx-launch and record what the prompt says
   and what appears in System Settings.
4. **Granting CTX.app reaches no tab.** Give CTX.app Full Disk Access; item 2 still fails closed.
5. **Scoped launcher.** `ctx make launcher named "fda-test" for /bin/ls at /tmp`; grant it Full
   Disk Access; `ctx run in "<tab>" "fda-test"` (no arguments) lists /tmp; an agent's
   `fda-test ~/Library/Mail` is refused; a person typing `fda-test ~/Library/Mail` in CTX.app or
   an attached terminal lists Mail. `ctx list tabs` shows "running with the permissions of fda-test"
   while it runs.
6. **Job control through a launcher.** Ctrl-C, Ctrl-Z and `fg` reach the program started by a
   launcher in a tab.
7. **Rebuild.** `ctx rebuild launcher "fda-test"` (ad-hoc): the old grant no longer applies until
   granted again; with `signed by "Developer ID Application: …"` it still applies.

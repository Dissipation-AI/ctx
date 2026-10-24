# ctx grammar

_Generated from `ctx-cli/src/grammar.rs`. Don't edit by hand._

Every `ctx` command is a sentence: a verb, then phrases, each introduced by its word, in any order.
The one unlabelled value is the verb's object. `the`, `a` and `an` are ignored before words ctx knows.
Help, errors and the activity log print the canonical order shown here.

A tab is its name in quotes, `tab <number>`, or `this tab`. A window is `window <number>` or `window "<name>"`.

## Seeing what's there

### `list`

List open tabs, closed tabs, a tab's commands, the activity log, launchers or agents.

```
ctx list tabs|closed tabs|commands|activity|launchers|agents [in "<tab>"] [by me|<agent>] [since <number> minutes|hours|days ago] [up to <number>] [as json|toon]
```

- `ctx list tabs`: Every open window and its tabs: number, name, folder, what's running, idle or busy, unread output.
- `ctx list closed tabs`: Saved tabs that aren't running any more. Newest 20; `up to 100` for more.
- `ctx list commands in "Dev server"`: Commands run in that tab: where, exit status, start time, duration. No output.
- `ctx list activity by claude since 1 hour ago`: The activity log: who did what to which tab, newest first.
- `ctx list launchers`: Scoped launchers ctx made, and which tabs are running them now.
- `ctx list agents`: The agents in config.toml: where each works, the account it runs as, what its sandbox gives it, whose tabs it may use.

### `read`

Print what's in a tab: the screen, recent lines, or commands with their output.

```
ctx read ["<tab>" | everything | last <number> lines | last command | last <number> commands | command <number>] [in "<tab>"] [with colors] [up to <number> lines] [as json|toon]
```

- `ctx read "Dev server"`: What's on screen in that tab right now, as plain text.
- `ctx read last 200 lines in "Dev server"`: The last 200 lines, including scrollback.
- `ctx read everything in "Dev server"`: All of it.
- `ctx read last command in "Dev server"`: Only the most recent command and its output.
- `ctx read last 3 commands in "Dev server"`: The last 3 commands with their outputs, exit statuses and times.
- `ctx read command 14 in tab 212`: One specific command and its output. Cite a line as `tab 212, command 14, line 3`.

### `find`

Search every tab, open and closed: names, commands and output.

```
ctx find "<text>" [in "<tab>"] [at <folder>] [by me|<agent>] [since <number> minutes|hours|days ago] [up to <number> results] [as json|toon]
```

- `ctx find "cargo publish"`: Search every tab, open and closed. Newest first, with surrounding lines.
- `ctx find "deploy" in "Dev server"`: …only in that tab.
- `ctx find "deploy" at ~/code/site by claude since 30 days ago`: …narrowed by folder, by who ran the command, and by date.

### `ask`

Have a language model read a tab and answer with the exact lines that support it.

```
ctx ask "<text>" [about "<tab>" or all tabs] [at <folder>] [by me|<agent>] [since <number> minutes|hours|days ago] [within <number> seconds|minutes|hours] [as json|toon]
```

- `ctx ask "where did this session discover why the auth test fails?" about tab 212`: A short answer plus the exact, checked lines that support it.
- `ctx ask "which command fixed the build?" about all tabs at ~/code/site since 7 days ago`: Ask about several tabs at once.

### `status`

Server version, uptime, memory per tab, socket path.

```
ctx status [as json|toon]
```

- `ctx status`: Server version, uptime, memory per tab, socket path.

## Opening, closing and arranging

### `open`

Open a window or a tab.

```
ctx open window|tab [in window <number or name>] [at <folder>] [named "<name>"] [as json|toon]
```

- `ctx open window`: A new window with one tab in your home folder.
- `ctx open window at ~/code/site`: …starting in that folder.
- `ctx open tab`: A new tab in the current window (the Agents window for agents).
- `ctx open tab in window 2 at ~/code/site named "Dev server"`: Any combination of window, folder and name. Prints the new tab's number.

### `close`

Close a tab or a window. Asks first if something is running.

```
ctx close "<tab>" or window <number> [without asking] [as json|toon]
```

- `ctx close "Dev server"`: Close that tab. Asks first if something is running.
- `ctx close window 2`: Close a window and all its tabs. Same rule.
- `ctx close "Dev server" without asking`: Don't ask.

### `rename`

Name or rename a tab or a window.

```
ctx rename "<tab>" or window <number> [to "<name>"] [as json|toon]
```

- `ctx rename "auth" to "Claude - auth refactor"`: Name or rename a tab.
- `ctx rename window 2 to "Work"`: Name a window.

### `move`

Move a tab to another window.

```
ctx move "<tab>" [to window <number or name>] [position <number>] [as json|toon]
```

- `ctx move "Dev server" to window 2`: Move a tab to the end of another window.
- `ctx move "Dev server" to window 2 position 1`: …and choose its place.
- `ctx move "Dev server" to new window`: Give a tab a window of its own.

### `clear`

Clear a tab's screen and scrollback (the saved record keeps everything).

```
ctx clear "<tab>" [as json|toon]
```

- `ctx clear "Dev server"`: Clear the screen and scrollback. The saved record keeps everything.

### `reopen`

Bring back a closed tab with its number, name, folder and environment.

```
ctx reopen "<tab>" [as json|toon]
```

- `ctx reopen tab 40`: Bring back closed tab 40 as tab 40, with its name, folder and environment.

### `forget`

Permanently delete a closed tab's saved record.

```
ctx forget "<tab>" [without asking] [as json|toon]
```

- `ctx forget tab 40`: Permanently delete a closed tab's saved record. Asks first.

## Doing things in a tab

### `run`

Run a shell command at a tab's prompt, wait for it, print its output and exit with its exit status.

```
ctx run [in "<tab>"] ["<text>"] [with colors] [up to <number> lines] [within <number> seconds|minutes|hours] [without waiting] [as json|toon]
```

- `ctx run in "Dev server" "npm test"`: The way to run a shell command: runs it at the tab's prompt, waits, prints its output, exits with its exit status.
- `ctx run in "Dev server" "npm run dev" without waiting`: Start a long-running command and return immediately.
- `ctx run in "Build" "cargo build" within 10 minutes`: Stop waiting after 10 minutes; the command keeps running in the tab.

### `type`

Type text into the program running in a tab, exactly as if typed.

```
ctx type "<text>" [into "<tab>"] [then press <key>] [as json|toon]
```

- `ctx type "print(df.head())" into "Python"`: Type into whatever program is running in the tab. Doesn't press Enter.
- `ctx type "yes" into "Dev server" then press enter`: …and press Enter.

### `press`

Press a key or a combination in a tab.

```
ctx press <key> [then <key>…] [in "<tab>"] [as json|toon]
```

- `ctx press ctrl-c in "Dev server"`: Press a key or combination: enter, escape, tab, up, ctrl-c, ctrl-d, f5…
- `ctx press escape then enter in "Editor"`: Several keys in order.

### `wait`

Wait for something to happen in a tab instead of polling.

```
ctx wait [until "<tab>" is idle|shows "<text>"|is quiet for <duration>] [within <number> seconds|minutes|hours] [as json|toon]
```

- `ctx wait until "Dev server" is idle`: Wait for the running command to finish. Prints its exit status.
- `ctx wait until "Dev server" shows "Listening on"`: Wait for that text to appear in the output.
- `ctx wait until "Dev server" is quiet for 2 seconds`: Wait until there's been no new output for 2 seconds.

### `show`

Bring a tab to the front in CTX.app.

```
ctx show "<tab>" [as json|toon]
```

- `ctx show "Dev server"`: Bring that tab to the front in CTX.app (opening its window if needed).

### `attach`

Connect this terminal to a tab (no GUI needed). Press ctrl-] to detach.

```
ctx attach [to "<tab>"]
```

- `ctx attach to tab 4`: Use a tab from this terminal. Press ctrl-] to detach; the tab keeps running.

## For agents

### `install`

Route an agent's shell tool through ctx tabs (hooks), or write ctx's instruction block.

```
ctx install hooks|instructions [for claude|codex|gemini|antigravity|cursor|pi|opencode|agents] [at <folder>] [as json|toon]
```

- `ctx install hooks for claude`: Installs ctx's Claude Code plugin for you (`claude plugin install`): Bash runs in a tab per session.
- `ctx install hooks for antigravity`: Installs ctx's Antigravity plugin (`agy plugin install`): run_command runs in a tab per conversation.
- `ctx install hooks for codex`: Also cursor, pi and opencode, each through its own plugin registry (Cursor: its hooks.json).
- `ctx install hooks for claude at ~/code/site`: Only for that project (Claude's local scope).
- `ctx install instructions for claude`: Writes to ~/.claude/CLAUDE.md, between ctx's markers.
- `ctx install instructions for codex`: Writes to ~/.codex/AGENTS.md.
- `ctx install instructions for gemini`: Writes to ~/.gemini/GEMINI.md.
- `ctx install instructions for agents at ~/code/site`: Writes to that project's AGENTS.md (or CLAUDE.md if that's what it uses).

### `remove`

Remove ctx's hooks or instruction block, or delete a launcher.

```
ctx remove hooks|instructions|launcher [for claude|codex|gemini|antigravity|cursor|pi|opencode|agents] [at <folder>] [as json|toon]
```

- `ctx remove hooks for antigravity`: Uninstall ctx's plugin; the agent's shell is its own again.
- `ctx remove instructions for claude`: Remove the block; nothing outside ctx's markers is touched.
- `ctx remove launcher "sysadmin"`: Delete a launcher, and remind you to remove its entry in System Settings.

### `mcp`

Serve ctx to an agent over MCP (stdin and stdout). Started by the agent, not typed.

```
ctx mcp
```

- `ctx mcp`: Run the MCP server: one `shell` tool whose commands run in a ctx tab.

## Permissions: scoped launchers

### `make`

Build a small signed launcher that holds a macOS permission for one program in one folder.

```
ctx make launcher [named "<name>"] [for <program>] [at <folder>] [allowing any arguments] [signed by "<name>"] [as json|toon]
```

- `ctx make launcher named "sysadmin" for ~/.local/bin/claude at ~/Agents/sys-admin`: Generate, build and sign a launcher for that program and folder.
- `ctx make launcher named "sysadmin" for ~/.local/bin/claude at ~/Agents/sys-admin allowing any arguments`: Pass arguments through.

### `rebuild`

Rebuild a launcher from the current template.

```
ctx rebuild launcher [as json|toon]
```

- `ctx rebuild launcher "sysadmin"`: Rebuild after a ctx update. Says whether a re-grant is needed.

## Other

### `restart`

Restart the ctx server (after an update). Open tabs are interrupted and can be reopened.

```
ctx restart server [without asking] [as json|toon]
```

- `ctx restart server`: Restart the server, e.g. after installing a new ctx. Asks first if tabs are open.

### `help`

Example sentences for every verb, or one verb in detail.

```
ctx help ["<text>"]
```

- `ctx help`: One page of example sentences.
- `ctx help run`: One verb in detail.

## Values

- "<tab>"
- window <number or name>
- <folder>
- <number>
- <number> seconds|minutes|hours
- <number> minutes|hours|days ago
- "<text>"
- <key>
- json|toon
- me|<agent>

Keys: enter, escape, tab, backspace, delete, space, up, down, left, right, home, end, pageup, pagedown, insert, f1–f12, any single character, with `ctrl-`, `alt-`, `shift-` or `cmd-` in front (`ctrl-c`, `shift-tab`).

## Reserved words

Tab and window names must contain a letter and can't be exactly one of these words:

`a`, `about`, `activity`, `agents`, `ago`, `all`, `allowing`, `an`, `antigravity`, `any`, `arguments`, `as`, `ask`, `asking`, `at`, `attach`, `by`, `clear`, `close`, `closed`, `colors`, `command`, `commands`, `cursor`, `day`, `days`, `everything`, `find`, `for`, `forget`, `help`, `hooks`, `hour`, `hours`, `idle`, `in`, `install`, `instructions`, `into`, `is`, `json`, `last`, `launcher`, `launchers`, `line`, `lines`, `list`, `make`, `mcp`, `me`, `minute`, `minutes`, `month`, `months`, `move`, `named`, `open`, `opencode`, `pi`, `position`, `press`, `quiet`, `read`, `rebuild`, `remove`, `rename`, `reopen`, `restart`, `results`, `run`, `screen`, `second`, `seconds`, `server`, `show`, `shows`, `signed`, `since`, `status`, `tab`, `tabs`, `the`, `then`, `this`, `to`, `today`, `toon`, `type`, `until`, `up`, `wait`, `waiting`, `week`, `weeks`, `window`, `with`, `within`, `without`, `yesterday`

## Exit status

0 on success, 1 for something you can fix (no such tab, a sentence that didn't parse), 2 if the server couldn't be reached, 3 if a time limit ran out. `ctx run` exits with the command's own exit status.

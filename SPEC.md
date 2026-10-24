# ctx — spec

`ctx` is the engine of CTX. It has two halves in one binary:

- **the server** (`ctx server`, normally started by launchd, never typed by hand), which owns every
  shell, its pseudo-terminal, its screen state and its scrollback; and
- **the CLI** (`ctx …`), which humans, scripts, AI agents, `bro` and CTX.app use to talk to it.

It replaces tmux for this workspace, and it is designed to be read by someone who has never typed
a shell command.

## Goals

1. **Readable.** Every command reads like an English sentence: `ctx run in "Dev server" "npm test"`,
   `ctx open tab in window 2`.
   An onlooker should understand what an agent is doing from the command alone.
2. **One way to do each thing.** No aliases, no abbreviations, no alternative flags.
3. **AI-first.** Every command can answer `as json`, with a stable, documented schema. Agents can
   wait for things to happen instead of polling.
4. **Small and fast.** Idle server < 40 MB with 15 tabs. Every CLI call returns in < 10 ms
   (excluding `wait`).
5. **Full terminal fidelity.** Complex TUIs (Ink/React TUIs, vim, htop, Claude Code) must work
   exactly as they do in Ghostty.
6. **Headless.** Fully usable with no GUI, inside any other terminal.
7. **Nothing is lost.** Every tab is saved as it goes: commands, output (with colours), folder,
   environment. Closed tabs can be read, searched and reopened later, including after a reboot.
8. **The agent's shell.** An AI agent such as Claude Code runs its commands in a ctx tab instead of
   a fresh throwaway shell per command. Its state persists, and a human can watch and audit
   every command and output in CTX.app, colours included.

## Non-goals

- Panes and splits. There are only **windows** and **tabs**. (CTX.app may draw tabs side by side;
  the server does not know or care.)
- Remote sessions over the network (v1). Use ssh inside a tab.
- tmux compatibility, key-table configuration, or a status bar.
- Linux (v1). Keep platform-specific code behind a module boundary so it stays possible.

## Model

```
server
└── window 1  "Work"
│   ├── tab 41  "CTX spec writing"          ~/Workspaces/charles-terminal-experience   (running: cargo build)
│   └── tab 42  "Claude - auth refactor"    ~/code/site                                (idle)
└── window 2
    └── tab 43  "Dev server"                ~/code/site                                (running: npm run dev)

closed (saved)
    tab 40  "Sysadmin agent"               ~/agents/sysadmin       closed 2026-10-09 23:10
    tab 39                                 ~                       closed 2026-10-09 18:02
```

- **Tab**: one pseudo-terminal running one process (normally your login shell), plus its saved
  record. Tab numbers start at 1 and are **never reused, ever**. The counter is persisted, so
  "tab 42" means the same tab today and next month, open or closed. There's no separate
  "session" concept: a closed tab is simply a tab that isn't running any more.
- **Window**: an ordered list of open tabs, with a number (same never-reused rule) and an optional
  name. A window with no tabs is closed automatically.
- Tabs can move between windows. Windows and tabs exist whether or not CTX.app is open.
- **Every tab is identified by its folder and, optionally, a name.** The folder is always shown and
  follows the shell's current folder. The name is free text a person or agent chooses so tabs can
  be told apart, e.g. `Claude - auth refactor` or `Dev server`. It is not a slug.
  - Allowed: letters (upper and lower case), digits, spaces, `-` and `_`. 1–64 characters. Leading
    and trailing spaces are trimmed. Anything else is rejected with a message saying which
    character isn't allowed.
  - Case is preserved for display and ignored for matching.
  - Must contain at least one letter, and can't be exactly one of ctx's sentence words (see
    Command reference). Then a name can never be mistaken for a number or a keyword.
  - Names must be unique among **open** tabs; closed tabs may share names (find them by number or
    `ctx find`).
  - Programs can set the terminal title (OSC 0/2), and it's shown as a secondary title, but it never
    replaces the name.
- In a command, a tab is written as its name in quotes or as `tab` + number: `"Dev server"`,
  `tab 42`. This is identification, not an alias.
- Inside a tab, the environment variables `CTX_TAB` and `CTX_WINDOW` are set, and commands that
  take a tab default to **this tab** when run inside CTX.

## Command reference

### Commands are English sentences

`ctx` commands are written the way you'd say them, not in the `program noun subcommand --flag`
shape that argument-parser libraries impose:

```
ctx run in "Claude - auth refactor" "cargo test"
ctx open tab in window 2 at ~/code/site named "Dev server"
ctx read last 3 commands in "Dev server"
ctx wait until "Dev server" shows "Listening on"
ctx close window 2 without asking
ctx list tabs as json
```

The grammar:

- **A verb comes first** (`run`, `read`, `open`, `close`, `wait`…). After it come **phrases**,
  each introduced by an English word: `in`, `into`, `at`, `to`, `named`, `running`, `until`,
  `by`, `since`, `within`, `with`, `as`, `without`.
- **Phrases can come in any order**, as in English. `ctx run "cargo test" in "Dev server"` means
  the same thing as `ctx run in "Dev server" "cargo test"`. Each phrase's leading word says what it
  is, so order never matters. Help, error messages and the activity log always print the
  **canonical** order (the one in the tables below), so there's still one way that's *shown*.
- **The one unlabelled value** is the verb's object: the command for `run`, the text for `type`,
  the key for `press`, the search text for `find`.
- **Filler words** `the`, `a` and `an` are ignored: `ctx read the last command in "Dev server"`.
- **No dashed flags.** Options are phrases: `as json` (not `--json`), `without asking` (not
  `--force`), `with colors`, `within 10 minutes` (a time limit), `up to 1000 lines`.
- If a sentence can't be parsed, `ctx` prints the closest valid sentence instead of a usage dump:

  ```
  $ ctx run cargo test in auth
  I couldn't tell which part is the command. Put it in quotes:
    ctx run in "auth" "cargo test"
  ```

### Written for tokens, not keystrokes

The interface is designed so language models read and write it reliably, not to save typing.
Terse notations like Perl or `tar -xzvf` are hard for models: each character carries a lot of
meaning, and a one-character slip changes everything. Ordinary words give a model room to
work. So:

- **Whole words, never abbreviations or symbols:** `within 10 minutes`, not `10m` or `-t 600`.
  `since 30 days ago`, `for 2 seconds`, `last 3 commands`. Keys use their common names
  (`ctrl-c`, `enter`, `escape`).
- **Each word means one thing everywhere.** `in` always introduces a tab, `at` always a folder,
  `to` always a destination, `as` always an output format.
- **Output uses words too:** `exit status: 1`, `took 2.4 seconds`, `tab 42 "Dev server" at
  ~/code/site`, never bare `[1]` or `2.4s` columns without headers. JSON keys are full words
  (`exit_status`, `started_at`).
- **Numbers are always labelled** (`tab 42`, `command 14`, `line 3`), so a number in a sentence is
  never ambiguous.
- **Bounded by default.** Every command that can return a lot of text has a default cap and says
  so when it applies (`showing the last 400 of 12,503 lines; ctx read everything in tab 42 for
  all`). Agents never get an unbounded dump by accident.

### When a command doesn't parse: assist

`ctx` has an optional **assist** step, on by default and easy to turn off, for sentences that
don't fit the grammar:

1. **Deterministic first.** Typos in verbs and keywords (edit distance), a missing pair of quotes,
   a tab name that's almost right, phrases in the wrong slot. If one clear fix exists, it's
   suggested with no model involved, instantly.
2. **Then `bro`, if it's installed.** Turning intent into a terminal command is exactly
   [`bro`](../bro/SPEC.md)'s job, so `ctx` asks it the way any program would: it pipes a request
   to `bro --json` (see [bro spec § Called by other tools](../bro/SPEC.md#called-by-other-tools)).
   The request holds the failed sentence, the parse error, a compact grammar summary (generated
   from the grammar table, kept under ~800 tokens so small models cope) and the open tab names and
   numbers. `bro` replies with a command and an explanation:
   - if the command is a `ctx` sentence that **re-parses**, it's shown as the suggestion (one
     retry with the new parse error if it doesn't);
   - otherwise only the explanation is shown, e.g. "ctx can't split a tab into panes; tabs can be
     shown side by side in CTX.app".
3. **Never executed.** Assist only answers. The suggestion is printed for the human or agent to
   run:

   ```
   $ ctx kill the dev server tab
   That isn't a valid ctx command. Did you mean something like:
     ctx close "Dev server"
   (Suggested by your language model. Nothing was run.)
   ```

   With `as json`: `{"error": "not_a_valid_command", "explanation": "...", "suggestion": "ctx close \"Dev server\""}`.
   The exit status is 1 either way.
- **`ctx` doesn't depend on `bro`.** They're separate tools, packaged separately. `ctx` looks for
  `bro` on `PATH` (or `assist_command` in config). If it's missing, slow (time limit 3 seconds) or
  fails, `ctx` shows the deterministic error, so assist never makes `ctx` hang or fail differently.
- Turn off with `assist = false` in `~/.config/ctx/config.toml`, or per call with the environment
  variable `CTX_ASSIST=off` (e.g. in scripts and CI).
- Assist requests are logged in the activity log like any other request.

### Naming a tab or window in a sentence

- **A tab** is its name in quotes, or `tab` and its number: `"Dev server"`, `tab 42`.
  (Single-word names work without quotes, since the shell passes them the same way.)
- **A window** is always `window` and its number or name: `window 2`, `window "Work"`.
- **This tab:** inside a CTX tab, commands that need a tab default to the one they're run from.
  `this tab` says it explicitly.
- So that names and sentence words never collide, a tab or window name **must contain a letter**
  (so `42` can't be a name) and **can't be exactly one of ctx's sentence words** (`tab`, `window`,
  `in`, `this`, `last`, `all`, …; the full list is in `ctx/docs/grammar.md`). `ctx rename` explains
  if you hit one.

Exit status: 0 on success, 1 for something you can fix (no such tab, unparseable sentence), 2 if
the server couldn't be reached, 3 if a time limit ran out. For `run`, the command's own exit status.

### Seeing what's there

| Command | What it does |
|---|---|
| `ctx list` | Every open window and its tabs: number, name, folder, what's running, idle or busy, unread output. |
| `ctx list closed tabs` | Saved tabs that aren't running any more: number, name, folder, who opened it, when it closed. Newest 20; `up to 100` for more. |
| `ctx list commands in "Dev server"` | Commands run in that tab: where (`user@host:path`), exit status, start time, duration. No output. |
| `ctx list activity` | The activity log: who did what to which tab, newest first. Narrow with `by claude`, `in "Dev server"`, `since 1 hour ago`. |
| `ctx read "Dev server"` | What's on screen in that tab right now, as plain text. |
| `ctx read last 200 lines in "Dev server"` | The last 200 lines, including scrollback. |
| `ctx read everything in "Dev server"` | All of it. |
| `ctx read last command in "Dev server"` | Only the most recent command and its output. |
| `ctx read last 3 commands in "Dev server"` | The last 3 commands with their outputs, exit statuses and times. |
| `ctx read command 14 in tab 212` | One specific command and its output. Commands are numbered per tab, starting at 1, and output lines are numbered within each command, so any line can be cited as `tab 212, command 14, line 3`. |
| `ctx ask "where did this session discover why the auth test fails?" about tab 212` | Have a language model read the tab and return a short answer plus the **exact lines** that support it, so the asker doesn't have to read the whole tab. See Asking about a tab. |
| `ctx find "cargo publish"` | Search every tab, open and closed: names, commands and output. Newest first; prints tab, line and surrounding lines. |
| `ctx find "deploy" in "Dev server"` | …only in that tab. |
| `ctx find "deploy" at ~/code/site by claude since 30 days ago` | …narrowed by folder, by who ran the command (an agent, or `by me`), and by date (any combination). |
| `ctx status` | Server version, uptime, memory per tab, socket path. |

`read`, `list commands in` and `find` work on closed tabs too, from the saved record.

`read` prints plain text by default; `with colors` keeps the styling. For a full-screen program
(vim, top, a TUI), `read` returns exactly what a person would see on screen.

### Opening, closing and arranging

| Command | What it does |
|---|---|
| `ctx open window` | A new window with one tab in your home folder. |
| `ctx open window at ~/code/site` | …starting in that folder. |
| `ctx open tab` | A new tab in the current window (window 1 when headless; the Agents window for agents). |
| `ctx open tab in window 2 at ~/code/site named "Dev server"` | Any combination of which window, which folder and a name. To start something in it, use `ctx run … without waiting`. |
| `ctx close "Dev server"` | Close that tab. Asks first if something is running. |
| `ctx close window 2` | Close a window and all its tabs. Same rule. |
| `ctx close "Dev server" without asking` | Don't ask. |
| `ctx rename "auth" to "Claude - auth refactor"` | Name or rename a tab. `ctx rename window 2 to "Work"` for a window. |
| `ctx move "Dev server" to window 2` | Move a tab to the end of another window. `to window 2 position 1` to choose the spot. |
| `ctx clear "Dev server"` | Clear the screen and scrollback (the saved record keeps everything). |
| `ctx reopen tab 40` | Bring back a closed tab, as tab 40, with its name, folder and environment. See Saved tabs. |
| `ctx forget tab 40` | Permanently delete a closed tab's saved record. Asks first. |

`open` prints the new tab or window number, so scripts can capture it.

### Doing things in a tab

| Command | What it does |
|---|---|
| `ctx run in "Dev server" "npm test"` | **The** way to run a shell command. Runs it at that tab's prompt, waits for it to finish, then prints its output and exits with its exit status. This is the verb agents use instead of a throwaway shell (see Agents). Options: `with colors`, `up to 1000 lines`, `within 10 minutes`. |
| `ctx run in "Dev server" "npm run dev" without waiting` | Start a long-running command (a server, a watcher, a build) and return immediately. Check on it later with `read`, `wait` or `find`. |
| `ctx type "print(df.head())" into "Python"` | Type text into whatever program is running in the tab (a REPL, a prompt asking y/n, a TUI), exactly as if typed. Doesn't press Enter. |
| `ctx type "yes" into "Dev server" then press enter` | …and press Enter. |
| `ctx press ctrl-c in "Dev server"` | Press a key or combination: `enter`, `escape`, `tab`, `up`, `ctrl-c`, `ctrl-d`, `f5`… |
| `ctx wait until "Dev server" is idle` | Wait for the running command to finish (prompt is back). Prints its exit status. |
| `ctx wait until "Dev server" shows "Listening on"` | Wait for that text to appear in new output. |
| `ctx wait until "Dev server" is quiet for 2 seconds` | Wait until there's been no new output for 2 seconds (useful for TUIs). |
| `ctx show "Dev server"` | Bring that tab to the front in CTX.app (opening its window if needed). |

`wait` gives up after 5 minutes unless told otherwise (`within 60 seconds`), exiting with 3.

`type` uses bracketed paste when the program has asked for it, so multi-line text arrives intact
and doesn't run line by line.

**`run` vs `type`, so there's one way to do each thing:** `run` is for shell commands at the
shell's prompt. `type` is for talking to a program that's already running. ctx knows which is in
front (from the shell-integration marks), so:

- `type … then press enter` at an idle **shell prompt** is refused:
  `To run a shell command, use: ctx run in "Dev server" "npm test"`.
- `type` *without* Enter at a shell prompt is allowed. It **pre-fills** a command for a person to
  review and run themselves, which is useful for an agent proposing something risky.
- `run` when a program other than the shell is in front is refused, with a pointer to `type`.

### Help

`ctx help` prints one page of example sentences, grouped like the tables above. `ctx help run`
shows one verb. Every error says what to type instead:

```
$ ctx read tab 9
There is no tab 9. Open tabs are 41, 42 and 43. `ctx list` shows them all,
and `ctx list closed tabs` shows ones that have been closed.
```

### Implementation notes

- Hand-written parser over `argv` (no `clap`). Each verb declares its slots and the word that
  introduces each one. Parsing is: take the verb, drop fillers, peel off labelled phrases in any
  order, and what's left is the object. Ambiguity is an error with a suggested rewrite, never a
  guess.
- The grammar is data (one table in `ctx-cli/src/grammar.rs`) from which `ctx help`,
  `ctx/docs/grammar.md` and the canonical form used in logs are all generated.
- Shell completion for zsh is generated from the same table and completes tab names in quotes.

## Structured output: `as json` and `as toon`

Every command can answer in three forms:

- **plain text** (default): readable sentences and labelled tables, for people and for agents
  reading transcripts;
- **`as json`**: for programs and scripts;
- **`as toon`**: [TOON](https://github.com/toon-format/toon) (Token-Oriented Object Notation),
  for agents. TOON writes a list of same-shaped records as one header line plus one row each,
  instead of repeating every key, which is much shorter for ctx's tabular answers (`list`,
  `list commands in`, `find`, `list activity`).

Notes on TOON, so it's chosen for the right reasons:

- The **token savings are real** for uniform lists (the format's own benchmarks report roughly
  40% fewer tokens than JSON). For nested or irregular data it saves little and can be worse, so
  ctx's output types are kept flat where that's natural.
- The **comprehension claims are thin.** The maintainers report a small accuracy edge over JSON
  (about 76% vs 75%), and an independent preprint found plain JSON still the most accurate when
  models *write* structured output. Models have seen far more JSON in training.
- So: `as toon` is offered and **advertised to agents** (in `ctx help` and the agent
  instructions), but the default for agents isn't decided until the agent
  evaluation (see Agents → Evaluation) compares plain text, JSON and TOON on real tasks.
- ctx only *emits* TOON and never parses it, so a small encoder written against the TOON spec is
  enough, with no dependency needed.

The schemas are versioned (`"schema": 1`) and documented in `ctx/docs/output.md`, generated from
the Rust types in `ctx-protocol`. JSON and TOON carry exactly the same fields. JSON example:

```json
{ "schema": 1,
  "windows": [
    { "number": 1, "name": "Work", "tabs": [
      { "number": 41, "name": "CTX spec writing", "folder": "/Users/c/Workspaces/charles-terminal-experience",
        "opened_by": "you", "title": "cargo build",
        "state": "busy", "running": "cargo build", "pid": 81234,
        "started": "2026-10-10T17:52:01Z", "unread_lines": 0, "size": [120, 40] } ] } ] }
```

`ctx read last N commands in … as json` returns structured command records
(`{number, command, where, ran_by, exit_status, started_at, finished_at, output}`) based on
shell-integration marks. This is what `bro` and agents use. The same request `as toon`:

```
commands[2]{number,command,where,ran_by,exit_status,started_at,finished_at}:
  13,cargo build,c@macbook:~/code/site,claude (pid 5512),0,2026-10-10T17:52:01Z,2026-10-10T17:52:40Z
  14,cargo test -p auth,c@macbook:~/code/site,you,101,2026-10-10T17:53:02Z,2026-10-10T17:53:11Z
```

(Outputs follow as separate blocks, since multi-line text isn't tabular.)

## Activity log and visibility

Any process running as your macOS user can connect (see Security). ctx records **who did
what**: which agent or person opened, read, typed into or ran a command in which tab. The
record is for searching and auditing, so it isn't shown everywhere. Plain-text views stay
uncluttered, and the information is in `as json`/`as toon` output, the activity log, and
search filters.

- Every request from a client that isn't CTX.app is logged: time, client (process name, pid, and
  the nearest recognisable ancestor, e.g. `claude (pid 5512) in tab 2`), verb, target tab. Typed
  text is logged in full, except a password (see Passwords). Read requests are logged too,
  without the returned content.
- The server resolves the client from the socket peer credentials (`LOCAL_PEERPID`,
  `LOCAL_PEERCRED`) and walks its parent chain to the nearest recognisable program (`claude`,
  `codex`, `gemini`, `agy`, CTX.app, a shell in tab 41…). It records name, pid and process start
  time, so two Claude sessions are told apart. This identity is **verified**: it comes from
  the OS, never from the client.
- An agent may also **declare** a label by setting `CTX_AGENT_NAME` (e.g. `Claude - auth
  refactor`) in the environment it runs `ctx` from. It's stored next to the verified identity and
  marked as claimed, never instead of it.
- **Every command in every tab records who ran it** (`ran_by`): `you` when it was typed on the
  keyboard in CTX.app (or in an attached terminal), or the agent's identity when it came through
  `ctx run`/`ctx type`. If both contributed (an agent typed it, you pressed Enter), both are
  recorded. This is kept in the saved record, so it's searchable forever:
  - `ctx find "migrate" by claude`: commands Claude ran that mention "migrate";
  - `ctx list activity by codex in "Dev server"`: everything Codex did to that tab;
  - `ctx list commands in tab 212 as toon`: includes the `ran_by` column.
- CTX.app shows a small badge on a tab when something other than you has typed into it or read it
  in the last minute; clicking it shows the matching log entries.
- Log: `~/Library/Application Support/ctx/activity.log` (JSON lines, rotated at 10 MB).
  `ctx list activity` shows it in readable form.

## Saved tabs

Every tab is recorded as it runs, and the record outlives the tab. No other terminal does this
well. Terminal.app's "restore windows" only brings back a screenful of text.

### What's saved, per tab

- Number, name (and renames), folders visited, which window, who opened it (you, or e.g.
  `claude (pid 5512)`), opened/closed times, terminal size changes.
- The **environment**: every exported variable when the shell starts, then only what changed
  after each command. The zsh integration reports changes from a `precmd` hook (a private OSC
  sequence) without forking, and only when something actually changed.
- Each **command**: text, folder, start and end time, exit status, and its **output with colours
  and styles**.
- Full-screen programs (vim, top, TUIs on the alternate screen) are saved as the command, exit
  status and **final screen**, not every frame. `top` running for a day would otherwise be
  gigabytes.
- Output that isn't inside a marked command (e.g. inside ssh, where our shell integration isn't
  running) is saved as plain timestamped scrollback.
- Output per command is capped (`saved_output_mb_per_command = 5`). Past that, the first and last
  2.5 MB are kept with a note of how much was dropped. This covers dev-server logs that run for days.

### How it's written (cheap on the SSD)

- One append-only file per tab: `~/Library/Application Support/ctx/tabs/<number>.ctxlog`.
  Records are length-prefixed and `zstd`-compressed.
- Written when a command finishes, and every 30 s while a long command keeps producing output.
  Never per keystroke or per line. No `fsync` per record. A crash loses at most ~30 s of output.
- Rough volume: a heavy day of development is tens of MB of output, a few MB compressed. That's
  negligible SSD wear.
- Closed tabs take **no RAM**. A small SQLite index (`index.sqlite`) holds tab metadata, command
  text and names, with full-text search on them. `ctx find` uses the index, then streams through
  the compressed output of candidate tabs.
- Retention: `keep_closed_tabs_days = 365` and `saved_total_gb = 5`. The oldest records are pruned
  first. `ctx forget tab N` deletes one immediately.

### Reopening

`ctx reopen tab 40` brings tab 40 back **as tab 40**:

- A new shell starts in the tab's last folder, with its saved environment re-exported. Variables
  that belong to the old process are skipped: `PWD`, `OLDPWD`, `SHLVL`, `TERM*`, `CTX_*`,
  `TMPDIR`, `SSH_AUTH_SOCK`, `XPC_*`, `__CF*`.
- The saved output appears above an event line (see Event lines):
  `[reopened 2026-10-10 18:30 after reboot · last active 2026-10-09 23:10]`.
- Up-arrow history in that tab is that tab's own commands (a per-tab zsh history file), and they
  are still appended to your global history as usual.
- If a command was still running when the tab ended (reboot, crash), the prompt is **pre-filled**
  with it, not run.
- Not restored in v1: unexported shell variables, functions and aliases defined by hand in that
  shell, background jobs.
- After a reboot or server crash, the tabs that were open are marked *interrupted*. On launch,
  CTX.app offers "Reopen 7 tabs from last time" (`reopen_on_launch = "ask" | "always" | "never"`).

### Event lines

Things that happen *to* a tab, as opposed to output from a program, are written into the tab as a
single dimmed line in square brackets. They're shown in CTX.app, included in `ctx read` and
`ctx run` output (so agents see them too), and kept in the saved record. They're the tab's
timeline:

```
[opened 2026-10-10 09:12 by claude (pid 5512)]
[reopened 2026-10-10 18:30 after reboot · last active 2026-10-09 23:10]
[reopened 2026-10-11 08:02 · last active 2026-10-10 19:45]
[reopened 2026-10-11 08:02 after server restart]
[renamed "auth" → "Claude - auth refactor"]
[moved to window 2 "Work"]
[ssh to deploy@web-1 started 14:03]
[ssh to deploy@web-1 ended 14:41 · connection lost]
[program cleared the screen — earlier output is kept in the saved record]
[closed 2026-10-10 19:45]
```

- The set of events is fixed and listed in `ctx/docs/events.md`; each has a JSON form
  (`{"event": "reopened", "at": …, "reason": "reboot"}`) in `as json` output.
- Event lines are drawn by the server *between* program output lines, never inside a program's
  screen. While a full-screen program is running, events queue up and are written when it exits.
- Programs can't produce event lines. They're stored as events in the record, not as text, so
  printing `[reopened …]` from a script makes ordinary output, styled as ordinary output.
- Agent `type`/`run` actions aren't event lines (they'd flood the tab). They're in the activity log
  and badge. The command itself is visible at the prompt anyway.

### Privacy

Saved records contain whatever was on screen and in the environment, including secrets. Files are
mode 0600 in your user's Application Support folder (encrypted at rest by FileVault). Patterns in
`dont_save_env` keep matching variable names out of the record entirely. They aren't restored,
and `reopen` lists them as "not restored". Reads of saved tabs by agents are logged like any other
read.

A secret a program prints (a token you `cat`) is saved like any other output. Rotating it is the
answer, not ctx. Redacting tokens and API keys from saved records is a later feature
(Phases, 13).

### Passwords

A password is never recorded: not in a tab's record, not in the activity log, not in CTX.app.

- **Keystrokes are never logged**, from CTX.app or an attached terminal. Records hold what the
  screen shows, and a password prompt shows nothing.
- **The server knows when a password is being read.** A program reading a password (`sudo`, `ssh`,
  `getpass`, `read -s`) turns echo off and reads a line. The server checks the terminal's mode
  after output and once a second, and puts it in screen updates. A line editor (zsh, readline)
  or a full-screen program reads keystroke by keystroke, so it doesn't count.
- **CTX.app turns on Secure Keyboard Entry** (`EnableSecureEventInput`) while it is active and
  the tab you're typing into is reading a password, and off again as soon as either stops. While
  it's on, no other process (an event tap, a key logger, a text expander) sees keystrokes. It
  isn't left on all the time, because that breaks text expanders and password-manager typing
  everywhere else.
- **Text an agent types is logged once it shows on screen.** `ctx type` text that goes to a
  password prompt is logged as `[hidden]` at once. Other typed text is logged when it appears on
  screen, and as `[hidden]` if it hasn't within 2 seconds. That covers prompts the terminal mode
  can't show: a remote `sudo` inside ssh, or a full-screen field that draws `***`. The entry keeps
  who, when and which tab, with `"typed_hidden": true` and the sentence
  `ctx type [hidden] into tab 3 then press enter`. Keys typed into a full-screen program that
  don't show (vim's normal mode) are hidden too, which is the safe side.
- Limits: Secure Keyboard Entry follows the local terminal mode, so a password prompt on the far
  side of ssh doesn't turn it on (the ssh login prompt itself does, since ssh reads it locally).
  An attached `ctx` in another terminal relies on that terminal's own Secure Keyboard Entry.

## Agents

The AI-first part of CTX: an agent works **in a tab**, like a person, instead of in an invisible
shell that's thrown away after every command.

### Why

Claude Code's Bash tool runs each command in a fresh shell. It does carry the **working
directory** over between calls, but nothing else: `export`ed variables, an activated virtualenv,
shell functions and aliases defined along the way, and anything interactive are gone on the next
call. The human sees the commands only inside the agent's own UI, without colours. With ctx:

- All state persists across commands: `export`, `cd`, `source .venv/bin/activate`, background
  servers, a REPL with data loaded.
- The human can open the agent's tab in CTX.app and watch every command next to its output, in
  colour, in real time or afterwards.
- The record is kept (Saved tabs), so a later agent can look up how an earlier one did something:
  `ctx find "deploy" by claude`, then `ctx read last 20 commands in tab 212`.

### `ctx run`: the agent's verb

```
$ ctx run in "Claude - auth refactor" "cargo test -p auth"
running 14 tests
...
test result: ok. 14 passed; 0 failed
$ echo $?
0
```

- Types the command at the tab's prompt (visibly, as if typed), presses Enter, waits for the shell
  to return to the prompt, then prints the command's output and **exits with the command's exit
  status**. Plain text by default; `with colors` keeps styling.
- Output is what the terminal showed: stdout and stderr interleaved, as a person sees them. A
  terminal can't separate the two, and that's accepted.
- Long output: prints at most 400 lines (`up to 1000 lines` to change it), as head + tail with a line saying how
  much was skipped and the exact `ctx read` command to see all of it.
- **No time limit by default.** `run` waits as long as the command takes. Agents' tools already
  impose their own timeouts, and if the caller gives up or is killed, the **command keeps
  running in the tab** and nothing is lost. The next call can pick it up with
  `ctx wait until "…" is idle` or `ctx read last command in "…"`. A caller that wants a limit
  says so (`within 10 minutes`). When it runs out, `run` prints the output so far plus
  `still running in tab 42; use ctx wait or ctx read`, and exits 3. The command is never killed
  by ctx.
- **Long-running things** (dev servers, watchers, long builds) use `without waiting`. It returns
  as soon as the command has started, with the tab number and how to check on it.
- **People** have no limits at all. Typing in a tab in CTX.app is just a terminal; `run` and its
  options exist only for programs driving ctx.
- Multi-line scripts: if the command argument is omitted, it's read from stdin (a heredoc), which
  avoids quoting trouble.
- Refuses, with a clear message, when:
  - the tab is busy running something else (`tab 43 is running "npm run dev"; open another tab or
    use ctx wait`);
  - someone has typed something at the tab's prompt but not pressed Enter (`someone is typing in
    tab 42`). This prevents an agent from mangling a human's half-typed command.
  - Two `run` calls racing on one idle tab: the server serialises them, and the second gets "busy".
- Needs shell integration (OSC 133 marks) to know where the command's output starts and ends. In a
  tab without it (inside ssh), `run` refuses and suggests `type` + `wait`.

### Where agent tabs go

When an agent (any client other than CTX.app, run outside a CTX tab) runs `ctx open tab` without
naming a window, the tab goes to a window named **Agents**, created if needed. Agent work doesn't
land in the window you're typing in. Agents are expected to name their tabs
(`ctx open tab named "Claude - auth refactor"`). Tabs opened without a name are shown by folder only.

### Teaching agents the rules: instruction files

Agents only use ctx if they're told to, and how. ctx writes a short, generated instruction
block into the files each agent reads at startup:

| Command | Writes to |
|---|---|
| `ctx install instructions for claude` | `~/.claude/CLAUDE.md` |
| `ctx install instructions for codex` | `~/.codex/AGENTS.md` |
| `ctx install instructions for gemini` | `~/.gemini/GEMINI.md` |
| `ctx install instructions for agents at ~/code/site` | that project's `AGENTS.md` (or `CLAUDE.md` if that's what the project uses) |

- The block sits between `<!-- ctx instructions: begin -->` and `<!-- ctx instructions: end -->`.
  Installing again **updates** it in place, `ctx remove instructions for claude` removes it, and
  nothing outside the markers is ever touched. A missing file is created.
- It's generated from the grammar table, so it always matches the installed `ctx`. It's
  deliberately short (under ~400 tokens), since it's paid for in every session:
  - run shell commands with `ctx run in "<tab>" "…"` in a tab you open and **name** for your task;
    use `without waiting` for servers;
  - check `ctx list` for existing tabs (like a running dev server) before starting another;
  - read with bounded commands (`read last 3 commands`, `find`, `ask`), never `read everything`
    on a big tab;
  - `as toon` / `as json` exist for structured answers;
  - leave the human's tabs alone unless asked.
- Phase 0 checks the exact instruction-file locations for each agent's current version, and the
  table is kept in one place in the code.

### Replacing agents' built-in tools (later, after evaluation)

Instructions *ask* an agent to use ctx; the deeper step is to make ctx **the** shell tool, so
agents can't fall back to a throwaway shell. Each agent has its own extension mechanism:

- **Claude Code:** a plugin with a `PreToolUse` hook on `Bash` that rewrites commands into `ctx run`.
  To make ctx the only shell instead, `permissions.deny: ["Bash"]` removes the Bash tool from
  Claude's context entirely and `ctx mcp` supplies `shell` (a hook that denies calls is pointless
  when the tool can be removed).
- **OpenAI Codex CLI:** the same kind of hook, through its plugin registry.
- **Google Gemini CLI / Antigravity:** an extension bundling the MCP server and the instruction file,
  plus their settings for excluding built-in shell tools.
- Exact mechanisms change between agent versions, so each is verified when this phase starts,
  not assumed from this spec.

**`ctx mcp` is only for agents whose shell can't be hooked** (decided 2026-10-23). It has one
tool, `shell` (`command`, optional `timeout`), named like any agent's shell so the agent isn't
primed to treat it as special. Each command runs in a tab named for the session, as with the
hooks; everything else ctx does is reached by running `ctx …` in that shell, so there is one way to
do each thing. The earlier design, a tool per verb (17) plus a catch-all, cost ~3k tokens of schema
per session and duplicated the CLI.

`ctx` itself stays agent-agnostic: the MCP server and CLI are the same for everyone. The
per-agent pieces live in `ctx/integrations/<agent>/`.

### Evaluation (gates tool replacement)

Replacing an agent's shell can slow it down, raise token spend, or lower output quality, so
it isn't shipped on by default until it's measured. `ctx/evals/agents/` holds a task suite (fix
a failing test, start a dev server and verify a page, debug from logs, a multi-step refactor,
pick up a task another agent started). Each agent runs it in configurations:

1. built-in shell (baseline);
2. built-in shell + ctx instructions;
3. ctx MCP (`shell`) with the built-in shell disabled;
4. same as 3, with plain text vs `as json` vs `as toon` outputs.

Measured per run: task success (scripted checks), wall-clock time, tokens in and out, number of
tool calls, and how often the agent misused ctx (unparsed sentences, unbounded reads). A
configuration becomes a default only if it's no worse on success and no more than ~10% worse on
time and tokens than the baseline.

### Sharing tabs between agents (and you)

Because tabs persist and have names, they're shared resources:

- One agent starts `npm run dev` in a tab named `Dev server`. Other agents read its logs
  (`ctx read last 50 lines in "Dev server"`), wait for it (`ctx wait until "Dev server" shows
  "ready"`), or restart it (`ctx press ctrl-c in "Dev server"`, then `ctx run in …`). Nobody starts
  a second copy on another port.
- A human can hand an agent a tab mid-task ("look at tab 41, the build is failing there"), and an
  agent can hand one back (`ctx show tab 42`).
- Long-lived helpers (a REPL with data loaded, a database shell, an ssh session to a server) are
  set up once and used by any agent, instead of every agent rebuilding that state.
- **Handing work between agents.** When you run out of tokens with one agent and switch to another
  (e.g. Claude Code → Antigravity), the new one doesn't dig through the old one's private session
  files. It runs `ctx list`, then `ctx read last 30 commands in "Claude - auth refactor"`, and
  continues **in the same tab**, with the same folder, environment, running dev server and history.
  Nothing about a tab is tied to the agent that opened it, unless agents are configured (below).
- All of it is in the activity log and badged in CTX.app, so it's always visible who touched what.

### Agents as principals (added 2026-10-23)

With `[agents.<name>]` in config.toml, sharing becomes explicit. Each agent is a principal; a tab belongs to whoever opened it (recorded in the tab's log, so it survives reopening). An agent sees and drives its own tabs, plus those of the principals in its `see_tabs_of` / `drive_tabs_of`; you see and drive everything. Requests that act on tabs an agent can't see are answered as if those tabs don't exist; commands it can see but not drive are refused with the reason. CTX.app's requests count only when they come from you.

Identity comes from the strongest evidence available: the process's account (`account`), its Seatbelt sandbox (checked against a per-agent marker file only that agent's profile can read), a launcher in its ancestry (`launcher`), an agent program in an agent's folder (`folders`, `default_agent`), then the tab it runs in. `CTX_AGENT_NAME` is a claim, honoured only when it names a configured agent and never to become you. An unsandboxed agent running as you can still lie by double-forking out of its ancestry; the sandbox and accounts are what make identity hold.

Enforcement: the sandbox profile (`ctx-protocol/src/agents.rs`) is applied by `ctx-launch --sandbox` before an agent tab's shell starts, or by the `ctx shim` an agent program's name runs in your tabs. It fails closed: a profile that can't be applied means nothing starts. Agents with accounts start through `ctx-helper` (root, launchd), which checks its own root-owned rules, `getpeereid` of the caller, and, for root, an Authorization from an administrator, then drops to the account irreversibly, applies the sandbox and execs. The README's "Agents: who may reach what" has the config.

Known limits: approving a root tab blocks the server while the dialog is up; an agent with write access to your home can still plant things you run later (repository hooks, shell files), so the dev profile keeps those read-only; an account agent needs its own login for its agent program.

## Asking about a tab

`ctx ask` lets one agent (or person) question another tab's history without loading it into its
own context window. It works like delegating research to a helper and getting back only the
conclusion and its sources.

```
$ ctx ask "where did this session discover the element-X bug, and what fixed it?" about tab 212

Answer:
  The cause was found in command 14 (rg "element_x" src/), which showed element_x being
  registered twice. Command 19 removed the duplicate registration, and command 21 (cargo test -p
  ui) passed afterwards.

Evidence (each line checked against the saved record):
  tab 212, command 14, line 2:   src/ui/panel.rs:88:    registry.add(element_x());
  tab 212, command 14, line 5:   src/ui/mod.rs:41:      registry.add(element_x());
  tab 212, command 21, line 9:   test result: ok. 37 passed; 0 failed

Coverage: read all 31 commands (2,140 lines). Confidence: high.
To see more: ctx read command 14 in tab 212
```

### How it works

1. **Narrow without a model.** The command list of the tab (text, folder, exit status, all short)
   is always included. Keyword hits from the question (`find`) mark the likely commands.
2. **Map.** The candidate output is cut into chunks that fit the model's budget, each line tagged
   with its `command N, line M` address. For each chunk the model returns only the addresses of
   relevant lines plus a few words on why (JSON). Chunks are independent and can run in parallel
   when the endpoint allows.
3. **Reduce.** One final call gets the question, the command list and only the selected lines,
   and writes the answer, citing addresses.
4. **Verify.** `ctx` checks every cited address exists, and prints the evidence lines **from the
   saved record, not from the model's output**. Citations that don't exist are dropped and
   counted ("2 citations could not be verified and were removed"). An answer left with no
   verified evidence is reported as unsupported.
5. **Report coverage honestly.** It says how much was read. If a budget ran out it says so ("read
   18 of 31 commands; the rest were skipped"), because "I didn't find it" is only meaningful when
   everything was read.

Options: `about all tabs at ~/code/site since 7 days ago` (several tabs, using the same filters as
`find`), `within 2 minutes` (default 1 minute), `as json`. Answers are cached per (tab, record
length, question), so asking again is instant until the tab gets new output.

### Why this is worth having, and its limits

- **What it buys.** An agent can learn from a 20,000-line tab by spending a few hundred tokens of
  its own context instead of all of it. Because the evidence is verbatim and verified, the caller
  can trust the quotes even when it can't fully trust the summary.
- **The summary is lossy.** A small local model will miss things or misjudge relevance. The
  evidence lines and the `ctx read command N` pointer let the caller check and dig in. Agents
  should use `ctx find` first when they know the exact text, since it's deterministic, instant
  and free. `ask` is for questions of meaning ("why", "where did it figure out", "what fixed").
- **Negative answers are weak.** "Not found" is only reported with full coverage, and even then
  is labelled as the model's judgement.
- **Prompt injection.** Tab output is untrusted text (web pages, logs, files an agent `cat`ed).
  It might contain instructions aimed at the summariser, and its answer then lands in another
  agent's context. Mitigations: the model's output is forced into the JSON shape (addresses plus a
  short answer), evidence comes from the record and not the model, the answer is labelled as
  model-written, and `as json` keeps answer and evidence in separate fields. This reduces the risk
  but doesn't remove it; the calling agent should treat the answer as data, not instructions.
- **Secrets.** Redaction (the same rules as `bro`, from the `shared/redact` library crate,
  compiled in) runs on everything sent to the model **and** on the evidence printed back, since
  the answer travels to another agent.
- **Speed.** A big tab on a small local model can take tens of seconds (many map calls). Bounded
  by the time limit and a cap on model calls (`ask_max_model_calls = 20`). A big-context model
  needs fewer, larger chunks, so it's usually faster as well as better.

### Its own model, not bro's

`ask` is a different job from `bro`'s. It reads and condenses large transcripts, while `bro`
writes one short command fast. So `ask` has **its own model configuration in `ctx`**, and doesn't
use `bro` at all:

- Any OpenAI-compatible chat endpoint, configured under `[ask]` in `~/.config/ctx/config.toml`.
  It's typically a larger local model with a big context, or a cloud model, since tabs can be long.
- No endpoint configured → `ctx ask` says exactly what to add to the config, and nothing else
  in `ctx` is affected.
- If the endpoint isn't local, `ctx` warns once that tab contents (after redaction) will be sent
  to it.
- Prompts live in `ctx/ctx-ask/prompts/`. HTTP is `ureq` (blocking, no async runtime), in the
  `ctx-ask` crate only, so the rest of `ctx` stays free of networking code.

## ssh

ssh inside a tab is just a program running in the tab, but it gets special care so remote work is
recorded as well as local work, and so a remote session can't wreck the tab.

### Recording remote commands

Every command in `list commands in`, `read last N commands in`, `find` and saved records is shown with where it
ran, the same for local and remote:

```
c@macbook:~/code/site$ npm test
deploy@web-1:/srv/app$ sudo systemctl restart app
deploy@web-1:/srv/app$ journalctl -u app -n 50
c@macbook:~/code/site$ git push
```

How the remote side gets marked, best first:

1. **Remote shell integration (default on).** When you run `ssh <host>` *interactively* (no
   remote command, stdin is a terminal), the zsh integration runs it as ssh with a small inline
   bootstrap as the remote command (the approach kitty's `kitten ssh` uses). The bootstrap
   detects the remote login shell, and for **bash or zsh** loads the user's normal rc files plus
   our OSC 133 / OSC 7 hooks, then hands control to that shell. It's sent inline, writes nothing
   permanent to the remote machine, and needs nothing installed there.
   - Not touched: `ssh host command`, `scp`, `sftp`, `rsync`, `git` over ssh, or a host whose ssh
     config sets `RemoteCommand`. Those run exactly as typed.
   - Remote shells it doesn't recognise (fish, plain `sh`, busybox, network gear) get no
     bootstrap; it falls back to 2.
   - Turn off globally (`ssh_integration = false`) or per host (`ssh_integration_skip = ["router",
     "*.prod"]`).
2. **Screen-based fallback.** Without marks, the server still records everything shown on screen.
   It finds commands by recognising the remote prompt (the line ending at the cursor when you press
   Enter, matched against the prompt seen before) and records `prompt + what's on that line`.
   It's best-effort and shown as such (`list commands in` marks these rows `~`).
   **Keystrokes are never logged**, only what the screen shows, so passwords typed at an echo-off
   prompt are never recorded.

With remote integration, `ctx run`, `ctx wait until … is idle` and `read last command` work in an ssh tab
too. An agent can run commands on a server through your ssh session, and you can audit them like
local ones. Remote environment variables are not saved (v1).

### Keeping the tab sane

Most of what makes ssh "do strange things" to a terminal comes from these, and each has a fix:

- **Unknown terminal type on the remote** (`'xterm-ghostty': unknown terminal type`, broken
  backspace, garbled vim). ctx reports `TERM=xterm-256color`, which every server already knows.
- **Modes left on after a disconnect.** If a connection drops while a remote program has turned on
  the alternate screen, mouse reporting, bracketed paste, application keypad, the kitty keyboard
  protocol or a hidden cursor, they stay on locally, and the tab looks broken (mouse clicks print
  garbage, arrows print `^[OA`). Whenever the local shell's prompt comes back (OSC 133 prompt
  mark), the server **resets every mode a program can leave on** to what the shell expects, and
  restores the tab's title. This applies to any program, not only ssh.
- **Wiping scrollback.** `clear` on many servers sends "erase scrollback" (`ESC[3J`). Programs
  aren't allowed to erase scrollback (`allow_programs_to_clear_scrollback = false`). The screen
  clears, an event line notes it, and history above stays. Only you can clear scrollback (⌘K or
  `ctx clear "<tab>"`), and even then the saved record keeps everything.
- **Frozen connections.** When an ssh tab gets no output for a while after you type, CTX.app shows
  a hint that `Enter ~ .` closes a hung connection. An event line records how the connection
  ended.
- **Nested tmux/screen on the server** work like any full-screen program, saved as their final
  screen. Commands run *inside* a remote tmux can't be recorded individually. Use ctx tabs instead,
  one ssh per tab.

### Reopening an ssh tab

A reopened tab that ended inside ssh comes back on your Mac with `ssh deploy@web-1` **pre-filled**
at the prompt, not run, so reconnecting is one Enter.

## Architecture

### Processes

- **One server per user**, started by launchd as a LaunchAgent (`com.ctx.server`). CTX.app
  registers it with `SMAppService.agent`. When the server isn't running, the CLI starts it through
  CTX.app if there is one (the app it's bundled in, else the one named on the first line of
  `<data>/server-registration`, which the app writes): `CTX --start-server`. Two registrations
  with one label make macOS refuse one at login. Without the app, the CLI registers it with
  `launchctl bootstrap`. The CLI must **never** start the server as its own child process, because the
  server would then inherit the calling terminal's macOS permissions (see Permissions).
- The server keeps running when CTX.app quits or crashes, so tabs survive a GUI crash and can be
  reattached. The GUI is detached from the tabs: quitting CTX.app leaves them running, and the next
  launch shows them again. With `keep_tabs_when_app_quits = false` in config, quitting closes all
  tabs instead (with Terminal.app's "processes are running" confirmation).
- The CLI is a short-lived client: connect, send one request, print, exit.

### Inside the server

- Event loop: a single-threaded `mio`/`polling` loop (no tokio, to keep idle memory and wakeups
  low). One thread owns all terminal state. PTY reads are non-blocking and handled on the loop.
- **Terminal emulation:** `libghostty-vt` via its Rust bindings (Ghostty's parser and screen
  model, proven against real-world TUIs; objects are single-threaded, which suits this design).
  Fallback if its API churns too much: `alacritty_terminal`. Pick one in the Phase 0 spike and hide
  it behind a `Terminal` trait.
- **PTY:** `openpty` directly (not `portable-pty`). Shells are started through `ctx-launch`
  (see Permissions), which needs macOS-specific spawn attributes.
- **Shell integration:** the server injects a tiny zsh integration (via `ZDOTDIR` shim that then
  sources the user's real `~/.zshrc`) that emits OSC 133 prompt/command/output marks and OSC 7 for
  the current folder. This is what makes `read last command`, `list commands in`, `wait until … is idle`, and the
  tab's folder possible. Without the marks (e.g. inside ssh), those features say so plainly
  instead of guessing.

### Scrollback and memory

Other terminals spend memory in two places: **GPU surfaces per tab** (avoided by CTX.app drawing
every tab through one surface per window; see the ctx-app spec) and **scrollback**. Scrollback
can be just as large: 15 tabs × 10,000 lines × 200 columns × ~16 bytes per cell ≈ 480 MB if
stored naively. So:

- Each tab keeps a small **hot** region (screen + ~1,000 lines) in the emulator's native format.
- Older lines are moved to a **cold** store: lines packed as text plus run-length-encoded styles,
  grouped into blocks of ~256 lines, compressed with `lz4` (fast) once a block is full.
  Typical shell output compresses 5–10×.
- Budgets are set in **megabytes, not lines**: `scrollback_mb_per_tab = 8`,
  `scrollback_mb_total = 200`. When the total is exceeded, the oldest blocks of the least recently
  viewed tabs are dropped first.
- Cold blocks stay in memory. A few MB of compressed text is cheap, and reading scrollback back
  from disk would just cost SSD reads and latency for nothing. (The on-disk saved record in Saved
  tabs is for keeping and restoring tabs, not for paging scrollback.)
- `read`, `find` and GUI scrolling decompress blocks on demand.
- Phase 0 must confirm the emulator lets us take lines as they leave its scrollback. If it
  doesn't, set its scrollback to the hot size and capture lines from our own hook.

### Protocol

- Unix domain socket at `$TMPDIR/ctx-<uid>/server.sock`. The directory is mode 0700 and the
  socket 0600.
- Length-prefixed messages encoded with `postcard` (compact serde). Types are defined once in
  `ctx-protocol` and shared by the CLI and CTX.app. The protocol carries a version, and the
  server refuses mismatched clients with a clear message.
- Request/response for CLI verbs. CTX.app additionally **subscribes** to the tabs it is showing
  and receives screen updates: changed rows only (damage tracking from the emulator), as runs of
  `(text, style)`, coalesced to at most one update per display frame (~8 ms on a 120 Hz screen).
  Tabs nobody is looking at cost no rendering work at all.
- Input from CTX.app (keys, mouse, paste, resize) travels on the same connection. Key encoding
  (including the kitty keyboard protocol) is done by the server's emulator, so the GUI and
  `ctx press` encode keys the same way.

### Terminal features required for v1

xterm-256color + truecolor, alternate screen, bracketed paste, focus events, mouse reporting
(SGR), synchronized output (mode 2026, needed by Ink/React TUIs and Claude Code to avoid flicker),
kitty keyboard protocol, OSC 7, OSC 8 hyperlinks, OSC 52 clipboard (writes allowed, reads
refused), OSC 133, wide characters, emoji and grapheme clusters, and reporting
`TERM=xterm-256color`, `TERM_PROGRAM=ctx`. Images (kitty graphics, sixel) are post-v1.

## Permissions

**Requirement:** the terminal grants nothing. Whether a process may read `~/Documents`, record the
screen or control other apps is decided by macOS for that process's signed binary. Giving
CTX.app access to something must not give it to the commands you run, and vice versa.

### How other terminals actually behave (researched 2026-10-10)

macOS privacy checks (TCC) look at a process's *responsible process*, and children inherit it.

- **Terminal.app, iTerm2 and Ghostty all behave the same way:** everything run inside them is
  attributed to the terminal app. Grant the terminal Full Disk Access and every command, agent and
  script run in it has Full Disk Access too.
- **Ghostty tried to fix this and backed out.** Issue
  [ghostty#9263](https://github.com/ghostty-org/ghostty/issues/9263) proposed exactly the mechanism
  below: a launch helper started with `posix_spawn` + `responsibility_spawnattrs_setdisclaim`,
  which then forks the shell (so `setsid` and `TIOCSCTTY` still work), with a proof of concept in
  #9274. It was closed as *not planned* in October 2025. The maintainer's reasoning: disclaiming
  "pushes the TCC permissions down to the shells". If a user then grants the shell (`/bin/zsh`)
  a permission to make things work, **any** process on the system can spawn a disclaimed shell
  and use that grant. A grant on a general-purpose shell is worse than a grant on a terminal app.
- There are reports of Apple planning changes to Full Disk Access (announced 2026-10-02, details
  unpublished, not verified here). Phase 0 re-checks against the current macOS.

### Evidence: per-binary permissions already work, in any terminal (tested 2026-10-10)

- **This Claude Code session** runs as `ghostty → login → zsh → claude → zsh`. Neither Ghostty
  nor `claude` has Full Disk Access, and `ls ~/Library/Mail`, `~/Library/Messages`,
  `~/Library/Safari` and `/Library/Application Support/com.apple.TCC` all fail with
  `Operation not permitted`.
- **Charles's sys-admin agent** (`~/Agents/sys-admin`) runs Claude Code inside Ghostty with Full
  Disk Access, and only that Claude. It's started by `bin/sysadmin-agent`, a ~100-line C
  *scoped launcher* (ad-hoc signed, identifier `com.c.sysadmin-agent`), which alone is granted
  Full Disk Access:
  1. Stage 1 re-spawns its own executable with `posix_spawn` +
     `responsibility_spawnattrs_setdisclaim(attr, 1)`, so stage 2 becomes its own responsible
     process instead of Ghostty.
  2. Stage 2 `chdir`s to the project and spawns `claude` **as a child, staying alive as its
     parent** (spawn, not exec). `claude` and everything it runs inherit stage 2 as their
     responsible process, so TCC checks *the launcher's* code signature.
  3. It forwards job control: it ignores SIGINT/SIGQUIT while waiting (they reach `claude`
     through the process group) and stops itself when `claude` stops, so Ctrl-Z/`fg` work.

So Ghostty's maintainers were right about the risk, but their conclusion doesn't follow.
Disclaiming is only dangerous when the grant lands on a **general-purpose** binary (a shell).
When it lands on a **narrow, purpose-built signed launcher**, you get exactly the "permissions per
signed binary" model, and it works today regardless of which terminal it runs in.

One caution from that same example: a launcher's grant is available to **anything that can run
the launcher**. `sysadmin-agent` passes its arguments straight through to `claude`, so any process
running as you could start an FDA-holding Claude with a prompt of its choosing. Narrow
launchers should fix their arguments, or check who's calling them (parent process, or an
interactive terminal with a person present).

### The underlying constraint

Anything that holds a permission can be driven by anything that can drive it. If CTX's shells
held Full Disk Access, every process that can talk to the ctx socket could use it with
`ctx run`, and by design that includes every agent. So **CTX's own processes and shells hold no
permissions**, and permissions live only on narrow, signed launchers chosen by the user.

### Design

1. **Nothing flows down from CTX.** The server is started by launchd (not by any app), and each
   tab's shell is spawned disclaimed. A tiny launcher (`ctx-launch`) uses the same two-stage
   technique as above, then forks the shell with `setsid`/`TIOCSCTTY`. `ctx-launch` holds no
   grants. CTX.app and the server request no privacy permissions and need none. Even if a user
   grants CTX.app something, it can't reach a tab.
2. **Fail closed.** Plain commands in a tab get only what needs no permission. A protected location
   gives `Operation not permitted`, and ctx's hint explains why. The hint actively warns *against*
   granting `zsh`, `bash`, `sh`, `python`, `node`, `perl`, `ruby` or a terminal app, because a grant
   on any of those is a grant to everything that can run them.
3. **Scoped launchers are the way to grant.** Exactly the sys-admin pattern: a small signed binary
   per purpose, holding the permission, staying as the parent of what it starts. ctx makes them
   first-class without owning them:
   - They work in a ctx tab exactly as in Ghostty: PTY, process groups and job control behave
     like any terminal's (covered by tests).
   - **Visibility:** ctx shows when a tab is running under a different responsible process than
     its shell, e.g. `ctx list` → `tab 44 "Sysadmin agent" … running with the permissions of
     sysadmin-agent`. The badge in CTX.app is distinct. The server reads this per foreground process
     (via the private `responsibility_get_pid_responsible_for_pid`, verified in Phase 0).
   - **ctx builds them for you** (see Making launchers below). ctx never grants anything
     itself; you grant the launcher in System Settings.
4. **Signing note.** Ad-hoc signed launchers (like `sysadmin-agent`) are identified by their code
   hash, so every rebuild needs a re-grant. A launcher signed with a Developer ID keeps its grant
   across rebuilds (TCC matches its designated requirement: team + identifier). The template
   supports both.
5. **Apps launched with `open`** get their own identity and grants through launchd, as always.

### Making launchers

`ctx make launcher` turns "this program, in this folder, with this permission" into a small,
signed, narrow binary, like `sysadmin-agent` but generated from a reviewed template, so nobody
has to write spawn-and-disclaim C by hand.

```
$ ctx make launcher named "sysadmin" for ~/.local/bin/claude at ~/Agents/sys-admin
Made ~/Library/Application Support/ctx/launchers/sysadmin/sysadmin (signed, ad-hoc).
It starts ~/.local/bin/claude in ~/Agents/sys-admin under its own macOS identity.
Arguments: accepted only when a person runs it at an interactive terminal, not from agents.

To give it Full Disk Access: System Settings → Privacy & Security → Full Disk Access → + →
choose the file above. (Opening that pane now.)
Start it in a tab with: ctx run in "Sysadmin agent" "sysadmin"
```

| Command | What it does |
|---|---|
| `ctx make launcher named "sysadmin" for ~/.local/bin/claude at ~/Agents/sys-admin` | Generate, build and sign a launcher for that program and folder. |
| `ctx make launcher … allowing any arguments` | Pass arguments through (the `sysadmin-agent` behaviour). ctx says plainly that anything able to run the launcher can then use its permission. |
| `ctx make launcher … signed by "Developer ID Application: …"` | Sign with a Developer ID so grants survive rebuilds (default: ad-hoc). |
| `ctx list launchers` | Launchers ctx made: name, program, folder, argument policy, signature, which tabs are running them now. Permissions can't be read without them, so it says where to check. |
| `ctx rebuild launcher "sysadmin"` | Rebuild after a ctx update to the template. Says whether a re-grant is needed (ad-hoc: yes). |
| `ctx remove launcher "sysadmin"` | Delete it, and remind you to remove its entry in System Settings. |

What every generated launcher does (the template, reviewed once, tested on every macOS release):

- **Two stages**, exactly as proven by `sysadmin-agent`: disclaim on the re-spawn, then spawn the
  program as a child and stay as its parent, forwarding job control (Ctrl-C, Ctrl-Z, `fg`).
- **Fixed program and folder**, compiled in as absolute paths. The program's path is checked
  against the code signature or hash recorded at build time, so swapping the file on disk
  doesn't inherit the permission.
- **Argument policy** (default: none from callers other than a person). It refuses arguments
  unless stdin is an interactive terminal *and* the call didn't come through an agent. Agents can
  still start it with no arguments.
- **Refuses to act as a generic shell**: no `-c`, no environment-variable program override, and a
  fixed `PATH`.
- **Visible in ctx**: the tab shows that it's running with the launcher's permissions, and the
  activity log records who started it.
- Builds with the system `clang` (Xcode Command Line Tools). If they're missing, `ctx make
  launcher` says how to install them. Source and build script are kept next to the binary, so
  you can read exactly what you're granting.

Phase 0 confirms on the current macOS: a disclaimed tab shell's behaviour on protected paths
(silent failure or prompt, and what appears in System Settings); scoped launchers inside ctx tabs
(Full Disk Access, Screen Recording); job control through a launcher; reading the responsible pid
of a tab's foreground process.

## Security

- Trust boundary: your macOS user account. Any process running as you can already ptrace your
  shells, so the socket, private to your user, is equivalent to tmux. Agents are allowed in by
  design, and they are **visible** (activity log + badges) rather than gated.
- `OSC 52` clipboard *reads* are refused (a program could otherwise read your clipboard silently).
- Programs can't change a tab's title to look like another tab's number.
- The server never runs commands on its own. It only spawns the shell/command a client asked for,
  and logs it.

## Configuration

`~/.config/ctx/config.toml`. The server reloads it on change.

```toml
shell = "/bin/zsh"                 # default: your login shell
scrollback_mb_per_tab = 8
scrollback_mb_total = 200
keep_tabs_when_app_quits = true    # false: quitting CTX.app closes every tab
log_reads = true                   # log agent read requests in the activity log

# saved tabs
keep_closed_tabs_days = 365
saved_total_gb = 5
saved_output_mb_per_command = 5
reopen_on_launch = "ask"           # ask | always | never
dont_save_env = []                 # e.g. ["*_TOKEN", "AWS_SECRET_*"]

allow_programs_to_clear_scrollback = false

# ssh
ssh_integration = true
ssh_integration_skip = []          # host patterns, e.g. ["router", "*.prod"]

# agents
run_max_lines = 400                # run has no default time limit
agent_window_name = "Agents"

# assist: suggest a fix for sentences that don't parse (uses bro if installed)
assist = true
assist_command = "bro"             # looked up on PATH; missing = deterministic suggestions only

# ctx ask: its own model, independent of bro
[ask]
endpoint = ""                      # e.g. "http://localhost:8080/v1" or a cloud provider
model = ""
api_key_env = ""                   # name of the env var holding the key, never the key itself
max_context_tokens = 32000
max_model_calls = 20
time_limit = "1 minute"
```

## Stack

| Concern | Choice | Why |
|---|---|---|
| Language | Rust, edition 2024, stable toolchain pinned in `rust-toolchain.toml` | speed, memory control, one language across the repo |
| Event loop | `mio` (or `polling`), single-threaded | no async runtime, low idle memory and wakeups |
| Terminal emulation | `libghostty-vt` (fallback `alacritty_terminal`), behind a `Terminal` trait | proven TUI fidelity; decided in Phase 0 |
| macOS APIs | `libc`, `nix` for PTY/process; small hand-written FFI for `responsibility_spawnattrs_setdisclaim`, `libproc`, `LOCAL_PEERPID` | few dependencies, explicit unsafe |
| Wire encoding | `serde` + `postcard` | compact, fast, no schema compiler |
| Saved records | `zstd` per record; `lz4_flex` for in-memory cold scrollback | zstd for ratio on disk, lz4 for speed in RAM |
| Index | `rusqlite` (bundled SQLite, FTS5) | one file, full-text search, no service |
| JSON / TOON output | `serde_json`; own TOON encoder | TOON encoding only, small |
| HTTP (`ctx ask` only) | `ureq` with rustls | blocking, no runtime; kept out of every other crate |
| MCP | the official Rust MCP SDK if it stays small, else a hand-written stdio JSON-RPC server | decided when phase 5 starts |
| Errors | `thiserror` in libraries, plain `Result<_, String>`-style messages at the CLI edge | error text is user-facing English |
| Logging | `tracing` to a file only when `CTX_DEBUG` is set; off otherwise | no idle cost |

Rules:

- **Every dependency is justified** in the PR that adds it, and checked for maintenance and known
  advisories (`cargo deny`, `cargo audit` in CI). The binary size and idle-memory budgets in CI
  catch heavy ones.
- No `tokio`, no `clap`, no GUI or web crates in `ctx`.
- `unsafe` only in the FFI modules, each block with a `// SAFETY:` comment.
- MSRV = the pinned toolchain; bumped deliberately.

## Crate layout

```
ctx/
  ctx-protocol/   message types, output schemas (JSON/TOON), versioning (shared with ctx-app)
  ctx-server/     event loop, PTYs, spawning, emulator wrapper, scrollback store, activity log
  ctx-cli/        sentence grammar table + parser, human and JSON output
  ctx-store/      saved-tab records (.ctxlog), SQLite index, search, retention
  ctx-mcp/        MCP server: one `shell` tool for agents without a hookable shell
  integrations/   per-agent instruction templates, plugins/extensions, hooks
  ctx-ask/        `ask`: narrowing, chunking, model client, citation verification, prompts
  ctx/            the single `ctx` binary (cli + `ctx server`)
  ctx-launch/     tiny launcher for tab shells: disclaimed spawn, setsid, controlling tty
  launchers/      template + builder for `ctx make launcher` (C source, build script, signing)
  shell/          zsh integration scripts (embedded into the binary at build time)
```

The sentence parser is hand-written (see Command reference → Implementation notes); no `clap`.

## Phases

0. **Spike (decides the open questions):** disclaimed tab shells, scoped launchers inside tabs,
   responsible-pid lookup (see Permissions); libghostty-vt vs
   alacritty_terminal (fidelity on Claude Code, vim, htop, an Ink app; ability to take lines leaving
   scrollback; memory per tab); measure baseline memory.
1. Sentence parser + grammar table, server, `list`, `open`, `close`, `read`, `type`, `press`, headless attach for testing
   (`ctx attach to tab 4` connects your current terminal to a tab, for debugging and for use without
   the GUI).
2. Shell integration: `list commands in`, `read last command(s)`, `wait`, `run`.
3. Cold scrollback store and memory budgets. `find`.
4. Activity log, `ctx list activity`, peer identification.
5. `ctx run` (incl. `without waiting`), the Agents window, `ran_by` attribution,
   `ctx install instructions for …`, then `ctx mcp`, then `as toon`.
6. Saved tabs: per-tab records, index, `list closed tabs`, `find` over closed tabs, `reopen`, `forget`, retention,
   event lines.
7. Launchers: template, `make`/`list`/`rebuild`/`remove launcher(s)`, responsible-process
   visibility in `list` and CTX.app. (Proven technique; can move earlier if wanted.)
8. ssh: mode reset at prompt, scrollback-clear protection, remote bootstrap (bash + zsh), screen
   fallback.
9. Subscription protocol for CTX.app.
10. Assist (deterministic suggestions first, then the model).
11. `ctx ask`, with an eval set of tabs + questions + expected evidence lines.
12. Agent evaluation suite, then per-agent tool replacement (`integrations/`), shipped per agent
    only once it passes the evaluation bar.
13. Later: redaction of saved records. Tokens and API keys in output are replaced with
    `[redacted]` in records, `read` and `find`, using the shared `redact` crate that `ask` already
    uses.

## Testing

**Unit tests** live next to the code (`#[cfg(test)]` modules) and run on every commit with
`cargo test`. Required coverage, by crate:

- `ctx-cli`: the sentence parser. Every grammar row, phrases in every order, fillers, quoting,
  reserved words, and each error with its suggested rewrite. Property tests (`proptest`) check
  that any valid sentence, re-printed in canonical form, parses to the same request.
- `ctx-protocol`: encode → decode round trips for every message, and that version mismatches
  are rejected. Snapshot tests (`insta`) pin the JSON and TOON output of every command.
- `ctx-store`: record write/read round trips, a crash mid-write (truncated file) recovers to the
  last whole record, retention pruning, index rebuild from records.
- `ctx-server`: scrollback hot → cold migration and budgets, mode reset at prompt, `ran_by`
  attribution, `run` refusal rules (busy, someone typing, wrong program in front), and event-line
  placement.
- `ctx-ask`: chunking stays within budget; citation verification drops non-existent addresses;
  redaction applied to prompt and evidence.
- shared `redact`: a corpus of must-redact and must-not-redact strings.

**Integration and system tests** (in `ctx/tests/`, run in CI on macOS runners):

- Golden-screen tests: feed recorded PTY sessions (vim, htop, Claude Code, an Ink app) to the
  server and compare `read` output to expected screens.
- CLI grammar tests: every example in `ctx help` is executed in CI against a test server, in
  canonical order and in shuffled phrase orders, and must give identical results.
- Memory test: open 15 tabs, run a fixed workload, assert server footprint is under budget.
- Latency test: keystroke-to-PTY and PTY-to-update timings under load (`yes` in another tab).
- Saved-tab tests: run a scripted session, kill the server with `kill -9`, restart, `reopen`, and
  compare folder, environment, history and event lines.
- ssh tests against a local `sshd` in CI (bash and zsh remote shells, and an unknown shell for the
  fallback).
- Permission tests (manual checklist per macOS release, since TCC can't be scripted reliably):
  the Phase 0 matrix, re-run on each new macOS version.

CI: GitHub Actions on macOS (Apple Silicon) runners: `cargo fmt --check`, `cargo clippy -- -D
warnings`, `cargo test`, `cargo deny check`, memory and binary-size budgets.

## Open questions

- Secure Keyboard Entry for a password prompt on the far side of ssh: the remote integration could
  report the remote terminal's echo mode.

- Should `ctx type` into a tab where a command is running (rather than the prompt) ask for
  confirmation when the caller is an agent? Default: no, but it's logged and badged.
- Exact wording of the `ctx` vocabulary should get a pass from a non-programmer.

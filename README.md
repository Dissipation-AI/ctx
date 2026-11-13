# ctx

`ctx` is the engine of CTX, an AI-first, low-memory terminal for macOS. One binary holds:

- **the server** (`ctx server`, started by launchd), which owns every shell, its pseudo-terminal,
  its screen and scrollback, and a saved record of every tab; and
- **the CLI**, which people, scripts and AI agents use to talk to it, in plain English sentences.

```
ctx open tab named "Dev server" at ~/code/site
ctx run in "Dev server" "npm run dev" without waiting
ctx wait until "Dev server" shows "Listening on"
ctx read last 3 commands in "Dev server"
ctx find "deploy" by claude since 30 days ago
ctx list tabs as toon
```

It replaces tmux for this workspace, and an agent's throwaway shell: agents run their commands in
named, persistent tabs that a person can watch and audit, and every tab is saved as it goes, so it
can be read, searched and reopened later, even after a reboot.

The full design is in [SPEC.md](SPEC.md). Generated references: the grammar
([docs/grammar.md](docs/grammar.md)), output schemas ([docs/output.md](docs/output.md)) and event
lines ([docs/events.md](docs/events.md)). What was measured and decided while building it:
[docs/phase0.md](docs/phase0.md).

## What's here

| | |
|---|---|
| `ctx-cli` | The sentence grammar (one table), parser, help, completion, plain-text answers |
| `ctx-protocol` | Messages, `as json` / `as toon` schemas, config and paths |
| `ctx-server` | Event loop, tabs, libghostty-vt emulation, scrollback store, activity log, CTX.app updates |
| `ctx-store` | Saved tabs (`.ctxlog` records), SQLite index, search |
| `ctx-ask` | `ctx ask` (map, reduce, verified evidence) and assist through `bro` |
| `ctx-mcp` | `ctx mcp`: one `shell` tool, for agents whose shell can't be hooked |
| `ctx-launch` | Starts each tab's shell under its own macOS identity |
| `launchers/` | The reviewed template for `ctx make launcher` |
| `shell/` | zsh and bash integration, the ssh bootstrap, zsh completion |
| `integrations/` | Hooks for Claude Code, Antigravity, Codex, Cursor, pi and opencode (`ctx install hooks for …`); the Gemini CLI extension; `ctx-skills`, agent skills for ctx and interactive programs |
| `evals/agents/` | The agent evaluation that gates those integrations |
| `scripts/budgets.sh` | Memory, latency and size budgets |

## Building

Rust 1.99 (see `rust-toolchain.toml`), plus what Ghostty's terminal library needs:

- [Zig 0.16](https://ziglang.org/download/) on `PATH` or in `ZIG`;
- network access on the first build (to fetch Ghostty's source), or `GHOSTTY_SOURCE_DIR` pointing
  at an extracted `libghostty-vt` source tarball.

```sh
cargo build --release      # target/release/ctx, ctx-launch and ctx-helper
cargo test
sudo scripts/install.sh "$USER"
```

ctx installs for the whole computer, so agents' accounts run the same `ctx` you do: `ctx` and `ctx-launch` in `/usr/local/bin`, the ctx helper (a root daemon, see below) under launchd or systemd, and the skills where Claude Code and Codex read them for every account. `sudo scripts/install.sh --remove` takes it all out again; agents' accounts, homes and spaces stay until you remove them with ctx.

## Trying it without touching your tabs

`CTX_HOME` puts everything (socket, saved tabs, config) in one folder:

```sh
export CTX_HOME=/tmp/ctx-try
ctx server &            # normally launchd starts it
ctx open tab named "Try"
ctx run in "Try" "echo hello"
```

## Using it with coding agents

ctx can sit between an agent and its shell tool: a hook rewrites each shell command into
`ctx run` in a tab named for the session, so the agent's terminal persists and every command is
recorded. The agent isn't told; no instruction file (CLAUDE.md, GEMINI.md, AGENTS.md) changes.
It installs through each agent's own plugin registry:

```sh
ctx install hooks for claude                 # for you; or antigravity, codex, cursor, pi, opencode
ctx install hooks for codex at ~/code/site   # for one project only
ctx remove hooks for claude                  # undo (also with `at`)
```

| Agent | For you | `at <folder>` |
|---|---|---|
| Claude Code | `claude plugin install` (user scope) | Claude's local scope |
| Antigravity | `agy plugin install` | the project's `.agents/plugins.json` |
| Codex | `codex plugin add` (trust it once in `/hooks`) | the project's `.codex/hooks.json` |
| Cursor CLI | `~/.cursor/hooks.json` (it has no plugin installer) | the project's `.cursor/hooks.json` |
| pi | `pi install` | `pi install -l` |
| opencode | `opencode plugin --global` | `opencode plugin` in the project |

New sessions pick it up. The installed hooks call this `ctx` by its full path, and the ctx server
must be running (launchd starts it). Agents that sandbox their shell (Antigravity, Codex, Cursor
with its sandbox on) block `ctx` from its server, so those commands run outside the sandbox with
your approval as the check. Each `integrations/<agent>/README.md` has the details.

### Skills: teaching agents to use ctx

`integrations/ctx-skills` holds two Agent Skills: `using-ctx` (getting around ctx, reading the messages that make an agent think its shell is broken, long jobs in their own tab, finding earlier work, arranging tabs for you) and `interactive-programs` (consoles and CLIs that need a terminal, setup wizards, REPLs, debuggers, ssh, adb and database shells, run in a tab and typed into). The agent loads them only when a task needs them.

They install on their own, separately from the hook, so you choose whether ctx changes what your agent loads:

```sh
ctx install skills for claude                 # for you; or antigravity, codex, cursor, pi, opencode
ctx install skills for cursor at ~/code/site  # for one project only
ctx remove skills for claude                  # undo (also with `at`)
```

`ctx install hooks for …` asks whether to add the skills too (yes by default) when you run it in a terminal; from a script or an agent it installs only the hook and prints the `ctx install skills` sentence.

| Agent | For you | `at <folder>` |
|---|---|---|
| Claude Code | `claude plugin install ctx-skills` (user scope) | Claude's local scope |
| Antigravity | `agy plugin install` | the project's `.agents/plugins.json` |
| Codex | `codex plugin add ctx-skills@ctx` | copied into the project's `.agents/skills` |
| Cursor CLI | copied into `~/.cursor/skills` | copied into the project's `.cursor/skills` |
| pi | `pi install` | `pi install -l` |
| opencode | copied into `~/.config/opencode/skills` | copied into the project's `.opencode/skills` |

`scripts/install.sh` also puts both skills where Claude Code (`/Library/Application Support/ClaudeCode/.claude/skills`, `/etc/claude-code/.claude/skills` on Linux) and Codex (`/etc/codex/skills`) load them for every account, agents' included; with those, the per-person install above is only for the other agents. Copies don't update with ctx; run `ctx install skills` (or the install script) again after updating. Without ctx installed, Claude Code can take them straight from GitHub: `claude plugin install ctx-skills --marketplace Dissipation-AI/ctx`. See [integrations/ctx-skills/README.md](integrations/ctx-skills/README.md).

### Agents without a hookable shell: `ctx mcp`

For an agent whose shell tool can't be hooked, `ctx mcp` is an MCP server (stdio) with one tool, `shell`, that stands in for it: `{command, timeout?}` in, output and `[exit status: N]` out. Each command runs in a tab named for the session (`Gemini 48211`: the client's name and the server's process id), opened in the server's folder on first use, so state persists and a person can watch, as with the hooks. A command still running at the timeout (default 120 seconds) keeps running, and the reply says how to wait for it. A single ctx sentence (`ctx list tabs`) runs as is; everything else ctx does, the agent reaches by running `ctx …` in that shell. The server's MCP instructions are the same block `ctx install instructions` writes. The Gemini CLI extension uses it; so does Claude Code with its Bash tool removed (`integrations/claude/README.md`). The hooked agents don't need it.

## Agents: who may reach what

An agent is an account of its own on the computer, in the group `ctx-agents`. The operating system decides what it may reach, with the same users, groups and access lists that keep people apart, and ctx keeps those details out of sight:

```sh
ctx make agent named "email-agent"         # an account, a private home, ~/Agents/email-agent links to it
ctx switch to agent "email-agent"          # a tab as that account, in its home; run claude, codex, … there
ctx allow agent "email-agent" to read ~/Context/career
ctx forbid agent "email-agent" to read ~/Context/career
ctx allow agent "sys-admin" to write agent "email-agent"   # sys-admin administers email-agent
ctx limit agent "email-agent" to 4 GB of memory
ctx show agent "email-agent"
ctx remove agent "email-agent" with its files              # its home goes to your Trash
```

**Homes are private.** Each agent's home is where every account's is (`/Users/<name>`, `/home/<name>` on Linux), readable only by the agent and you. It holds the agent's own logins (`claude`, `codex`, `pi` … each signed in as that account), its instructions (`CLAUDE.md`, `AGENTS.md`) and `ctx-agent.toml`, its colour and limits, which belong to you and its administrators, never to the agent.

**Spaces are shared.** A space is a folder for the agents that work together on something, and a group the OS checks. Only its members (and you) can use it:

```sh
ctx make space named "job-search"          # /Users/Shared/Spaces/job-search (/srv/spaces on Linux)
ctx add agent "email-agent" to space "job-search"
ctx take agent "email-agent" out of space "job-search"
ctx list spaces
ctx remove space "job-search" with its files
```

Every tab has the spaces folder in `CTX_SPACES`; `~/Agents/Spaces` links to it. A new member's tabs opened after it joined can use the space.

**Your files** stay closed to every agent until you share a folder or file, and only for as long as you do. Sharing opens only the path to it: whatever sits beside it on the way stays closed, including files made there later.

**Tabs.** An agent's tabs belong to it and run as its account. It sees and drives only its own tabs, plus those of agents whose homes it may read (see) or write (drive). No agent ever sees or drives yours, restarts the server, or reopens your session's tabs; a copy you make of an agent's tab stays the agent's. Every activity log line names who did it (`email-agent: claude (pid 5512) …`).

**The ctx helper** (`ctx-helper`) is the only part of ctx with root. It answers only the account in its root-owned rules (the one the ctx server runs as), makes and removes agents' accounts and spaces, and starts a tab only as an agent's account: one in `ctx-agents`, standard, never yours or an administrator's. An agent's shell reaches the server through a door of its own (`CTX_SOCKET`), and `open`, `pbcopy` and `pbpaste` there reach your session through ctx.

## Licence

Apache-2.0.

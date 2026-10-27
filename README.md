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
cargo build --release      # target/release/ctx and target/release/ctx-launch, side by side
cargo test
```

Install both binaries in the same folder (`ctx` starts tab shells through `ctx-launch`).

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

`integrations/ctx-skills` holds two Agent Skills: `using-ctx` (getting around ctx, reading the messages that make an agent think its shell is broken, long jobs in their own tab, finding earlier work, arranging tabs for you) and `interactive-programs` (consoles and CLIs that need a terminal, setup wizards, REPLs, debuggers, ssh, adb and database shells, run in a tab and typed into). They work with or without the hook.

```sh
claude plugin install ctx-skills --marketplace Dissipation-AI/ctx
```

Other agents: see [integrations/ctx-skills/README.md](integrations/ctx-skills/README.md).

### Agents without a hookable shell: `ctx mcp`

For an agent whose shell tool can't be hooked, `ctx mcp` is an MCP server (stdio) with one tool, `shell`, that stands in for it: `{command, timeout?}` in, output and `[exit status: N]` out. Each command runs in a tab named for the session (`Gemini 48211`: the client's name and the server's process id), opened in the server's folder on first use, so state persists and a person can watch, as with the hooks. A command still running at the timeout (default 120 seconds) keeps running, and the reply says how to wait for it. A single ctx sentence (`ctx list tabs`) runs as is; everything else ctx does, the agent reaches by running `ctx …` in that shell. The server's MCP instructions are the same block `ctx install instructions` writes. The Gemini CLI extension uses it; so does Claude Code with its Bash tool removed (`integrations/claude/README.md`). The hooked agents don't need it.

## Agents: who may reach what

Each agent is a principal of its own, named in `~/.config/ctx/config.toml`. Its tabs belong to it; it sees and drives only those, plus the tabs of anyone listed in `see_tabs_of` / `drive_tabs_of` (`"you"`, another agent, or `"*"`). Every activity log line names the agent (`dev: claude (pid 5512) …`), including commands its hooks rewrote. `ctx list agents` shows the whole setup.

```toml
default_agent = "dev"            # agent programs started outside every agent's folders

[agent_base]                     # every agent's sandbox gets these too
read = ["~/.zshrc", "~/.gitconfig", "~/.cargo", "~/Library/Keychains"]
write = ["~/.claude", "~/.claude.json", "~/.cache"]

[agents.dev]
folders = ["~/Workspaces"]       # claude, codex, agy, … started here are dev
write = ["~"]
deny = ["~/Context", "~/Agents", "~/Library", "~/.ssh"]
read = ["~/.zshrc", "~/.ssh/known_hosts"]
see_tabs_of = ["you"]
# account = "vibe-daemon"        # run as its own macOS account (needs the ctx helper)

[agents.sysadmin]
launcher = "sysadmin"            # whatever runs through this launcher is the sysadmin
folders = ["~/Agents/sys-admin"]
sandbox = false
```

How an agent is recognised, strongest first: the account a process runs as, the sandbox it's in, a launcher in its ancestry, an agent program in an agent's folder, the tab it runs in. A declared `CTX_AGENT_NAME` counts only when it names a configured agent, and never upgrades a process to you.

**The sandbox** (on unless `sandbox = false`). Your home is closed, then the agent's `read`, `write` and `deny` paths are given back. The most specific path wins whatever its kind, so `write = ["~"]` with `deny = ["~/.ssh"]` and `read = ["~/.ssh/known_hosts"]` means what it says. Outside your home everything stays readable and only temporary folders are writable. ctx's own files, Apple Events and `open` are always closed. A tab for the agent starts its shell inside the sandbox; an agent program you start in one of your tabs (ctx puts a small shim for each of `agent_programs` first on PATH) applies it to itself before it starts. macOS privacy protection (TCC: Full Disk Access, browser data) still applies on top: the sandbox can only take away.

**Accounts.** With `account = "<name>"`, the agent's tabs run as that macOS account, started by the ctx helper (`ctx-helper`), a small root daemon. It accepts requests only from the account the server runs as, and only for the agents its own root-owned rules (`/Library/Application Support/ctx/helper.toml`) list, so nothing running as you can widen them. An agent that runs as root needs an administrator's approval (password or Touch ID) each time a tab starts. The agent's shell reaches the server through `/private/tmp/ctx-agents-<uid>/server.sock` (`CTX_SOCKET`). An agent program you start in your own tab runs in a new tab of the agent's, shown in your terminal until it exits.

```sh
cargo build --release -p ctx-helper
sudo scripts/install-helper.sh c dev=vibe-daemon sysadmin=sys-ops-monkey
sudo scripts/install-helper.sh --remove
```

An account only helps if it can't read your files: close your home (`chmod 700 ~`) and give back what each account needs with ACLs, and give each account its own login for its agent program (`claude setup-token` as that account).

## Licence

Apache-2.0.

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
| `ctx-mcp` | `ctx mcp`: the verbs as MCP tools |
| `ctx-launch` | Starts each tab's shell under its own macOS identity |
| `launchers/` | The reviewed template for `ctx make launcher` |
| `shell/` | zsh and bash integration, the ssh bootstrap, zsh completion |
| `integrations/` | Hooks for Claude Code, Antigravity, Codex, Cursor, pi and opencode (`ctx install hooks for …`); the Gemini CLI extension |
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

## Licence

Apache-2.0.

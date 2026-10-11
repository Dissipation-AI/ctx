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

The full design is in [SPEC.md](SPEC.md). The grammar is in [docs/grammar.md](docs/grammar.md)
(generated), the output schemas in [docs/output.md](docs/output.md).

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

## Licence

Apache-2.0.

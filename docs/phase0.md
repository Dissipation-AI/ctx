# Phase 0 findings

Measured 2026-10-11 on an M4 Pro, macOS 27.0.1, Rust 1.99, ghostty-vt 0.7.0 (Ghostty 1.3.2-main).
These decide the open questions SPEC.md leaves to Phase 0. Re-run on each new macOS release
(`docs/permissions-checklist.md`, `scripts/budgets.sh`).

## Terminal emulation: libghostty-vt

Chosen over `alacritty_terminal`. The `ghostty-vt` crate (a maintained fork of `libghostty-vt`
pinned to a newer Ghostty) builds with Zig 0.16 and exposes everything ctx needs: a terminal with
events instead of callbacks, formatting rows as plain text or SGR text, tracked row anchors
(pins), OSC 133 prompt semantics per row, key and paste encoding (kitty keyboard protocol
included), render state with dirty rows for CTX.app, snapshots, search and scrollback compression.
It sits behind the `Terminal` trait in `ctx-server/src/terminal.rs`.

Memory per tab (Ghostty's own scrollback, 120 columns): about 0.1 MB for a fresh tab, about
1.2 MB with 1,000 lines, about 10 MB with 10,000 lines. So the spec's design holds: keep ~1,000
"hot" lines in the emulator and move older lines to ctx's compressed store.

**Taking lines as they leave scrollback.** There's no callback for it, and Ghostty prunes in whole
pages (~400 KiB), which at narrow widths can hold rows not yet copied out. ctx therefore gives the
emulator no line limit, copies each row once as it enters history (a pin on the last row copied),
and then trims the emulator back to the hot size itself, only ever removing rows already copied.
Plain rows are copied in one formatting call per batch; only styled rows are formatted one by one.

**Golden screens.** vim, less, a Python REPL, an Ink-style redrawing TUI (synchronized output,
cursor-up redraws) and a synthetic full-screen monitor (alternate screen, scroll region, wide
characters) render identically to a real terminal (`ctx-server/tests/golden.rs`). Claude Code
itself was not recorded (it needs a signed-in session); the Ink stand-in covers its rendering
technique.

## Event loop: poll(2), not kqueue

macOS's kqueue doesn't report readiness on pseudo-terminal devices (tmux works around the same
thing), so `mio` and `polling` can't watch tab terminals. The server uses a small `poll(2)` loop
(`sys::Poller`). With a few dozen descriptors this costs nothing measurable. This replaces the
spec's "`mio` (or `polling`)" choice; everything else about the loop (single thread, no async
runtime) is as specified.

## Memory and latency (`scripts/budgets.sh`, release build)

| Budget | Spec | Measured |
|---|---|---|
| Server with 15 tabs, each with 2,000 lines of output, idle | < 40 MB | 3 MB |
| `ctx list tabs` round trip (median) | < 10 ms | 3 ms |
| `ctx run … "true"` while another tab runs `yes` (median) | (keystroke latency under load) | 28 ms |
| Server after a 3,000,000-line flood | – | 3–7 MB |
| `ctx` binary / `ctx-launch` binary | – | 6.1 MB / 0.3 MB |

Two things made the flood numbers: each tab reads at most 32 KB per loop turn (so one flooding tab
can't starve the others), and the server runs with `MallocSpaceEfficient=1` (set when it starts),
without which macOS keeps tens of megabytes of freed blocks cached after a flood. A command's
saved output is held compactly (one buffer plus line lengths) and written in 256 KB pieces during
a flood.

Throughput of a flood is about 230,000 lines a second (`yes`), limited by copying rows out of the
emulator; ordinary build and server output is far below that.

## Permissions (TCC)

Run with the server started by launchd (`ctx` registered it with `launchctl bootstrap`):

- Every process in a tab has `ctx-launch` (ad-hoc signed, no grants) as its responsible process
  (`responsibility_get_pid_responsible_for_pid`); the server is its own responsible process. A
  shell in Ghostty has Ghostty as its responsible process, as the spec found.
- Full Disk Access locations fail closed in a tab: `~/Library/Mail`, `~/Library/Messages`,
  `~/Library/Safari`, the Photos library and `/Library/Application Support/com.apple.TCC` all give
  `Operation not permitted`, with no prompt.
- **Not closed: `~/Desktop`, `~/Documents` and `~/Downloads` were readable from a tab with no
  prompt.** Files & Folders protection didn't apply to the disclaimed, unbundled `ctx-launch`.
  A likely fix to try: give `ctx-launch` a bundle identity (an embedded `Info.plist` with its own
  `CFBundleIdentifier`) so macOS asks, and the person can say no. That test raises permission
  dialogs, so it's left for a person at the machine (`docs/permissions-checklist.md`, item 3).
- Scoped launchers: built, signed and run in tabs; arguments are refused when an agent is anywhere
  in the call (`ctx-server`'s `WhoRan` answers inside tabs). Granting one Full Disk Access is a
  manual step (checklist item 5).

Claude Code asks again for permissions after each update because each version is a new file
(`~/.local/share/claude/versions/<version>`), even though its Developer ID signature
(`com.anthropic.claude-code`, team `Q6L2SF6YDW`) stays the same. Launchers sit at a fixed path and
check the program's signature requirement rather than its hash, so they keep working across
updates.

## Shells

zsh tabs load ctx's integration through a `ZDOTDIR` shim, then the user's own files. bash tabs
(also covered: CI runners and some people log in with bash) start with an rc file that reads the
login files and adds the same marks; macOS's bash 3.2 has no `PS0`, so command starts come from a
DEBUG trap there. Other shells get no marks; `run` says so and suggests `type` + `wait`.

## ssh

Tested against a user-level `sshd` on localhost: interactive sessions get the inline bootstrap and
remote commands are recorded with the remote shell's folder; skipped hosts fall back to commands
found on screen (`~` in `list commands`). `ctx/tests/cli.rs` runs this in CI.

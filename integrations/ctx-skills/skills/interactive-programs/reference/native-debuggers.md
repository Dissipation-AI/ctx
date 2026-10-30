# gdb, lldb, rust-gdb, core files

On macOS use lldb (gdb isn't signed for debugging there). On Linux gdb is usually the better default.

## One-shot first

Often one call is enough. lldb runs `-k` commands after a crash; `-o` commands after `run` are skipped when it crashes:

```
lldb -b -o run -k bt -k "frame variable" ./crash
gdb -q -batch -ex run -ex bt -ex "info locals" --args ./crash input.txt
gdb -q -batch -ex bt ./prog core.1234
```

Use a persistent tab when you will step, set breakpoints iteratively, or inspect many frames.

## lldb in a tab

```
ctx open tab named "lldb - crash" at ~/code/proj
ctx run in "lldb - crash" "lldb --no-use-colors ./crash" without waiting
ctx wait until "lldb - crash" is quiet for 2 seconds
ctx type 'run' into "lldb - crash" then press enter
ctx wait until "lldb - crash" shows "stop reason" within 1 minutes
ctx type 'bt
frame variable
up
frame variable
script print("DONE" + "-1")' into "lldb - crash" then press enter
ctx wait until "lldb - crash" shows "DONE-1" within 20 seconds
ctx read last 40 lines in "lldb - crash"
```

- `run`, `continue`, `next`, `step`, `finish` hand the terminal to the program. Put them last in a batch, or alone. Lines after them reach the program's stdin and are lost.
- Marker: `script print("DONE" + "-N")`.
- `breakpoint set -f file.c -l 42`, `breakpoint set -n func -c 'i == 7'`, `watchpoint set variable x`, `register read`, `memory read -c 32 ptr`, `expr -- some_call(1)`.
- `quit` asks `[Y/n]` while a process is alive: `process kill` first, or answer `y`.
- Attach: `lldb -p <pid>`. Interrupt a running program with `ctx press ctrl-c in "lldb - crash"`; it stops in the debugger, not the shell.

## gdb in a tab

```
ctx run in "gdb - server" "gdb -q --args ./server --port 8080" without waiting
ctx type 'set pagination off
set confirm off
set print pretty on
break handle_request if req->len > 4096' into "gdb - server" then press enter
ctx type 'run' into "gdb - server" then press enter
```

- `set pagination off` and `set confirm off` first, or `--More--` and `(y or n)` prompts stall the tab. Put them in `~/.gdbinit` only if the user wants that.
- Marker: `printf "%s-%d\n", "DONE", 1`.
- `bt full`, `info locals`, `frame 3`, `p *node`, `p arr[0]@10`, `x/16xb ptr`, `info threads`, `thread apply all bt`.
- `rust-gdb` / `rust-lldb` for Rust: pretty-prints `Vec`, `String`, `Option`.
- Remote: `gdbserver :2345 ./prog` in one tab (local or over ssh), `gdb -q ./prog` then `target remote host:2345` in another.

## Program output vs debugger output

The program's stdout and the debugger's prompt share the tab. If the program prints a lot, run it with output redirected (`run > /tmp/out.txt`) so reads show only the debugger.

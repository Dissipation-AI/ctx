---
name: persistent-terminals
description: Drives interactive and long-lived terminal programs through ctx tabs that keep their state between calls - Python/IPython REPLs, pdb, gdb, lldb, ssh sessions, adb shell and logcat, psql/sqlite3/mysql/redis-cli, node, and dev servers. Covers when a persistent terminal beats a one-shot command, setting variables and functions once instead of repeating long paths, sending several commands in one call, waiting for output without sleep or races, reading back only what was just printed, using several tabs at once and passing context between them, running several jobs in one tab (`&`, `jobs`, `wait`, ctrl-z and `bg`), and handing a tab to the human for passwords. Use when a task involves a REPL, a debugger, a remote or device shell, a database shell, a server that must keep running, repeated commands with the same long paths, or any program that asks for input.
---

# Persistent terminals with ctx

A ctx tab is a real terminal that stays alive between your tool calls, keeps its folder, variables, functions and running program, and is recorded and visible to the user. The `using-ctx` skill covers ctx itself. This skill is about using what a persistent terminal makes possible.

## When to use one

One call that answers the question is still best: `python -c`, `gdb -batch -ex bt`, `lldb -b -o run -o bt`, `psql -c`, `ssh host 'cmd'`, `adb shell cmd`, `git --no-pager`. Use a persistent tab when any of these is true:

- **State is expensive to rebuild**: a dataframe or model loaded in a REPL, a debugger stopped at the interesting frame, a logged-in ssh session, a device connection, a database with a transaction open.
- **You will make many small queries against the same state**: inspecting variables, stepping, poking at a remote box. Ten one-shot calls that each reload a 2 GB file waste minutes.
- **The program only works interactively**: prompts, TUIs, `ssh` that asks for a password, installers.
- **It has to keep running**: servers, watchers, `logcat`, `tail -f`, training jobs. It should outlive your session, and the user should be able to stop it and run a variant.
- **The user should be able to watch or take over.**

## Set things up once

Claude Code's own shell forgets variables and functions between calls, so long paths get repeated in every command. A ctx tab keeps them. Spend the first call on setup:

```
ctx run in "Build - site" 'export S=/private/tmp/claude-501/…/scratchpad P=~/code/site; cd $P && source .venv/bin/activate'
ctx run in "Build - site" 'pytest $P/tests -x -q > $S/pytest.log; tail -5 $S/pytest.log'
```

Single-quote the command so `$S` reaches the tab instead of being expanded by the calling shell. Functions work too: `t(){ pytest -x -q "$@" 2>&1 | tail -20; }` once, then `t tests/test_auth.py`. Tabs agents open already have pagers off (`PAGER=cat`, `GIT_PAGER=cat`, `MANPAGER=cat`…), so `git log` or `man` won't sit in `less` waiting for `q`.

Variables live only in that tab. To start another tab with the same setup, copy it: `ctx open tab like "Build - site"` gives a fresh shell with the same folder, exported variables and history (a venv's `PATH` comes along; functions and unexported variables don't). To bring variables into a tab that already exists, write them to a file and source it:

```
ctx run in "Build - site" 'export -p | grep -E " (S|P)=" > $S/env.sh'
ctx run in "Tests - site" 'source /private/tmp/…/scratchpad/env.sh'
```

## The loop: send, wait, read

`ctx run` is for shell commands at a prompt. Once a program owns the tab, use `type` and `press`:

```
ctx run in "Python - sales" "PYTHON_BASIC_REPL=1 python3 -q" without waiting
ctx wait until "Python - sales" is quiet for 1 seconds
ctx type 'import pandas as pd
df = pd.read_parquet("/data/sales.parquet")
print(df.shape, df.dtypes.to_dict())
print("DONE" + "-1")' into "Python - sales" then press enter
ctx wait until "Python - sales" shows "DONE-1" within 2 minutes
ctx read last 20 lines in "Python - sales"
```

**Batching.** One `type` can carry many lines; each is entered in order. Batch everything that returns straight to the program's prompt. This replaces 5 tool calls with 1.

**Rules that keep batches from going wrong.** Each of these was seen in testing:

1. **Check what owns the tab before typing.** `ctx list tabs` shows `busy: <program>` or `idle`. `run` refuses a busy tab, but `type` goes to whatever is there. If a debugger exited mid-batch, the remaining lines run as shell commands.
2. **End a batch at any line that hands the terminal to the program**: `run`, `continue`, `next`/`step` over code that reads stdin, `ssh`, a nested REPL. Lines after it go to the program, not the debugger, and are lost, including your marker. Send that line alone, wait for the stop, then send the inspection batch:

   ```
   # wrong: bt, frame variable and the marker go to the program's stdin
   ctx type 'run
   bt
   frame variable' into "lldb - crash" then press enter

   # right
   ctx type 'run' into "lldb - crash" then press enter
   ctx wait until "lldb - crash" shows "stop reason" within 1 minutes
   ctx type 'bt
   frame variable
   script print("DONE" + "-1")' into "lldb - crash" then press enter
   ```
3. **Make the "done" marker impossible in your input.** `shows` matches the running command's output so far, case-insensitive. A whole REPL session is one command, so an old prompt or an old marker matches at once. The echo of what you typed also counts. So compute a fresh marker each round: Python `print("DONE" + "-3")`, pdb `!print("DONE" + "-3")`, lldb `script print("DONE" + "-3")`, gdb `printf "%s-%d\n", "DONE", 3`, SQL `select 'DONE' || '-3';`, shell `echo DONE-$((1+2))`.
4. **Meta-commands go in their own `type`.** sqlite3 merged `.mode box` into the SQL that preceded it in a fast batch. The same applies to psql `\` commands, IPython `%magics` and `!shell` lines.
5. **Python 3.13+ REPL takes a multi-line `type` as one paste** and waits on `...` for one more Enter. Start agent-driven REPLs with `PYTHON_BASIC_REPL=1`, which enters line by line. Indented blocks still need a blank line after them.
6. **Some exits ask first.** lldb `quit` with a live process asks `[Y/n]`; so do psql in a transaction and many installers. If a wait times out, `ctx read "<tab>"` (the screen) before anything else.

**Waiting.** Never `sleep`. Use one of these:

- `ctx wait until "<tab>" shows "<marker>" within <limit>`: you control the output.
- `ctx wait until "<tab>" is quiet for 2 seconds`: quick commands, unknown output.
- `ctx wait until "<tab>" is idle`: the program exited back to the shell.

**Reading.** `ctx read last N lines in "<tab>"` after a batch. `ctx read "<tab>"` for the current screen of a full-screen program (htop, less, vim, a TUI installer). `ctx read last command in "<tab>"` returns the whole REPL session as one command, which is useful for a summary but large; cap it with `up to 60 lines`. Redirect bulky output to a file in the program (`df.to_csv(f"{S}/out.csv")`, `> $S/log`) and read only what you need.

**Keys.** `ctx press ctrl-c in "<tab>"` interrupts, `ctrl-d` ends input or exits, `escape`, `q` for pagers, `up` for history. `ctx press escape then enter in "<tab>"` for sequences.

## Several jobs in one tab

Job control works in a ctx tab as in any zsh:

```
ctx run in "Fetch - data" 'for r in us eu ap; do ./fetch.sh $r > $S/fetch-$r.log 2>&1 & done; jobs'
ctx run in "Fetch - data" 'wait; tail -n 3 $S/fetch-*.log' within 30 minutes
```

- `cmd &` returns the prompt at once, so the tab reads `idle` while jobs run; `jobs` lists them and `wait` blocks until all are done. Send each job's output to its own file, or their lines interleave on the screen.
- Use this for a batch of similar short jobs. Something you or the user will watch, stop or restart (a server, a training run) gets its own tab instead, where its output, exit status and ctrl-c belong to it alone.
- **Rescuing a job you started in the foreground:** `ctx press ctrl-z in "<tab>"` suspends it and frees the prompt, then `ctx run in "<tab>" 'bg; jobs'` lets it carry on in the background. If the line was a list (`a; b`), zsh runs `b` straight away once `a` is suspended.
- `kill %1` stops job 1; `fg` brings it back to the front (then the tab is busy again).
- `nohup` and `disown` are rarely needed: the tab's shell doesn't end with your session. Jobs started with `&` do end when the tab is closed.

## Several terminals at once

Open a new tab when:

- a program will own the terminal for a while (server, REPL, debugger, ssh, logcat);
- two things need to run at once: a server and the client hitting it, a program and the debugger attached to it (`debugpy`/`gdbserver`), a device's `logcat` and the shell that triggers the bug;
- you need a different machine or account: one ssh host per tab;
- the work is unrelated to what a tab already holds. Don't run ad-hoc shell commands in a tab where a REPL with data loaded is waiting.

Stay in an existing tab when the state you need is already there. `ctx list tabs` first: a REPL, ssh session or server someone already started is usually what you want.

Name tabs for what they hold (`Python - sales df`, `ssh slopslinger`, `Logcat - firetv`, `lldb - crash`). For a task with several tabs, give it a window: `ctx open window at ~/code/site named "Crash hunt"`, then `ctx open tab in window "Crash hunt" named "…"`.

**Passing context between tabs:**

- **Files are the clipboard.** Write values, logs and data in one tab (`$S/…`), read or source them in another. Don't copy long output through your own context to retype it.
- **Copies** of a tab (`ctx open tab like "<tab>"`) or sourced env files for variables, as above.
- **Read across tabs.** `ctx read last 30 lines in "Dev server - site"` while working in `Tests - site`; `ctx wait until "Logcat - firetv" shows "FATAL"` while driving the app from another tab.
- **Search across tabs.** `ctx find "Traceback" since 1 hour ago` finds errors in any tab, open or closed. `ctx read command 14 in tab 212` pulls one earlier result back.

## Humans in the loop

- **Passwords, 2FA and passphrases are the user's.** Get the program to its prompt, `ctx show "<tab>"`, tell the user what to type there, then `ctx wait until "<tab>" shows "<text that only appears after login>" within 5 minutes`. Never type a secret yourself.
- **Prompts you can't judge** (`Enable API cloudscheduler? [y/N]`, a destructive confirmation): ask the user. Don't leave the tab sitting there unmentioned, because it blocks everything else in it.
- **Hand-backs.** When you stop, say which tabs are still running what, by name and number, and what state each is in (`lldb - crash`: stopped in `sum()` at crash.c:3, `n` is NULL).

## Cleaning up

Quit REPLs and debuggers you started for one question (`quit`, `exit()`, `ctrl-d`; answer any confirmation), and close their tabs with `ctx close "<tab>" without asking`. Keep long-lived helpers the user will want (a server, an ssh session, a REPL with data loaded) and list them in your summary. The tab's record stays searchable after it closes.

## Program notes

Read the one you need:

- Python, IPython, pdb, debugpy: [reference/python.md](reference/python.md)
- gdb, lldb, rust-gdb, core files: [reference/native-debuggers.md](reference/native-debuggers.md)
- ssh, remote shells, sudo, tmux on the far side: [reference/ssh.md](reference/ssh.md)
- adb shell, logcat, devices: [reference/adb.md](reference/adb.md)
- psql, sqlite3, mysql, redis-cli, node: [reference/databases-and-repls.md](reference/databases-and-repls.md)

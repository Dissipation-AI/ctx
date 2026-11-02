# Several tabs, job control, shared setup

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

Name tabs for what they hold (`Python - sales df`, `ssh build-box`, `Logcat - tv`, `lldb - crash`). For a task with several tabs, give it a window: `ctx open window at ~/code/site named "Crash hunt"`, then `ctx open tab in window "Crash hunt" named "…"`.

**Passing context between tabs:**

- **Files are the clipboard.** Write values, logs and data in one tab (`$S/…`), read or source them in another. Don't copy long output through your own context to retype it.
- **Copies** of a tab (`ctx open tab like "<tab>"`) or sourced env files for variables, as above.
- **Read across tabs.** `ctx read last 30 lines in "Dev server - site"` while working in `Tests - site`; `ctx wait until "Logcat - tv" shows "FATAL"` while driving the app from another tab.
- **Search across tabs.** `ctx find "Traceback" since 1 hour ago` finds errors in any tab, open or closed. `ctx read command 14 in tab 212` pulls one earlier result back.

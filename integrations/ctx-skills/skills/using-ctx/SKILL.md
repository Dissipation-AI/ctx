---
name: using-ctx
description: Reads and drives ctx tabs other than your own - the named, recorded shells people watch in CTX.app. Use when the task names a ctx tab or mentions ctx or CTX.app, when you must read, stop or restart a program in a tab that someone else opened, when you look for commands that were run earlier, or when a shell call returns a ctx message such as `Tab N "…" is running …; open another tab`.
---

# Working with ctx tabs

ctx is a terminal whose server keeps every shell as a **tab** with a number and a name. Tabs keep their folder, variables and running program, and every command and its output is recorded. The `ctx` CLI takes English sentences, never flags: `ctx help` lists them, `ctx help <verb>` gives one verb's grammar.

## Your own shell

If ctx's hook is installed, your shell tool already runs each command in a tab named for your session (`Claude 4db0312f`, `Antigravity 8c1e…`). The tool reports "a pre-tool hook changed the arguments" each time; that is the hook, and it's expected. Plain commands work as usual and the tab keeps your folder and exported variables between calls. A background command gets its own copy of the tab, so a server you start keeps running after you finish. You don't need ctx sentences for ordinary work.

## Sentences for other tabs

Quote tab names; `tab 4` for a number.

```
ctx list tabs                                    every tab, what it runs (busy/idle), unread output
ctx read last 40 lines in "Dev server - site"    recent output, scrollback included
ctx read last 3 commands in "Dev server - site"  commands with their output, exit status and time
ctx press ctrl-c in "Dev server - site"          stop what it is running
ctx wait until "Dev server - site" is idle within 30 seconds
ctx run in "Dev server - site" "bun run dev" without waiting
ctx wait until "Dev server - site" shows "Local:" within 1 minutes
ctx open tab named "Tests - site" at /abs/folder
ctx find "Traceback" since 1 hour ago           search every tab, open or closed
```

Chain sentences with `&&` to save round trips. `run` waits for the command and prints its output (`within 10 minutes` caps the wait; the command keeps running). A tab keeps the folder it was left in, so use absolute paths.

### Restarting a program in someone's tab

Read why it failed, fix the code, then restart it in the same tab, so the user's tab keeps serving:

```
ctx read last 40 lines in "Dev server - site"
ctx press ctrl-c in "Dev server - site" && ctx wait until "Dev server - site" is idle within 30 seconds
ctx run in "Dev server - site" "<the command it was running>" without waiting && ctx wait until "Dev server - site" shows "<its ready line>" within 1 minutes
```

`ctx read last 3 commands in "<tab>"` shows the exact command it was started with.

## More

- A ctx message makes the shell look broken (`Tab N … is running`, output cut short, `Operation not permitted`, a refused name): [reference/troubleshooting.md](reference/troubleshooting.md)
- Claude Code auto mode denies or questions a ctx call: [reference/auto-mode.md](reference/auto-mode.md)
- Long jobs in their own tab, reusing what earlier agents ran, keeping output small, arranging tabs and windows for the user: [reference/layout-and-records.md](reference/layout-and-records.md)
- Driving a REPL, debugger, console or wizard: the `interactive-programs` skill.

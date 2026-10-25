---
name: using-ctx
description: Explains how to work inside ctx, the terminal whose server owns every shell as a persistent, named tab that people watch in CTX.app. Covers how ctx may be wired into the agent's own shell tool, reading the messages that make the shell look broken ("Tab N is running …; open another tab", output cut off by `within`, "Operation not permitted", wrong folder, auto-mode denials on ctx commands), running long jobs in their own tab instead of background shells, finding what earlier agents and the user already ran, and arranging tabs and windows for the human. Use whenever `ctx` is installed, a command mentions a ctx tab, the user mentions ctx, CTX.app, tabs or "the Claude tab", or a shell call fails in a way that doesn't match the command.
---

# Working in ctx

ctx is a terminal whose server owns every shell. Each shell is a **tab** with a number and a name, grouped in windows, shown in CTX.app, recorded (commands, output, exit status, folder), searchable and reopenable after it closes. The `ctx` CLI takes English sentences: `ctx run in "Dev server" "npm test"`. `ctx help` lists every sentence; `ctx help <verb>` gives one verb's grammar.

Start with `ctx list tabs`: every window and tab, what each is running (`busy: <program>` or `idle`), unread output, and which agent owns it.

## How your shell may be wired

Find out which applies before deciding anything is broken:

- **Bash hook** (`ctx install hooks for claude`): each Bash call is rewritten to `ctx run in "Claude <session id>" '<command>'`. Your commands run in that tab, one at a time, and it keeps its folder, variables and history between calls. A call that is a single ctx sentence (`ctx …` with no `;`, `&&`, `|`, redirect or `$(…)`) is not rewritten; it runs as written.
- **`ctx mcp`** (`mcp__ctx__shell`): same idea, the tab is named for the client, e.g. `Claude 48211`.
- **Neither**: your shell is your own and you reach tabs by calling `ctx` yourself.

A tab named `Claude <8 characters>` in that list, or a message starting `Tab N "Claude …"`, means the hook is on.

## Keep your session tab free

The session tab runs one thing at a time. Use it only for commands that finish quickly and that you wait for: reads, edits, short builds and tests. Anything that runs long, waits for input, or that you want to keep going while you work goes in **another tab**: `ctx open tab named "<what>"`, then `ctx run in "<what>" "…" without waiting`. This includes a Bash call with `run_in_background`, and anything that might prompt (`ssh`, `gcloud`, `sudo`, installers).

If your session tab gets stuck anyway, you can free it yourself. Don't ask the user to. Single ctx sentences aren't routed through the busy tab:

```
ctx read last 30 lines in "Claude 4db0312f"
ctx press ctrl-c in "Claude 4db0312f"
```

Read first. If it is a job worth keeping, leave it and open another tab. If it is a prompt or a hang, press ctrl-c (twice if needed; `ctx press ctrl-d` or `q` for some programs), then re-run the long part in its own tab.

## When the shell seems broken

| What you see | What it means | What to do |
| --- | --- | --- |
| `Tab N "Claude …" is running "<cmd>"; open another tab` | Your session tab is still busy: an earlier background call, a call that outlived its timeout, or a program waiting for input (password, `[y/N]`, pager). Every later hooked call gets this until it is free. | Free it yourself (see "Keep your session tab free"): `ctx read last 30 lines in tab N`, then `ctx press ctrl-c in tab N`, or answer a safe prompt with `ctx type "n" into tab N then press enter`. A password: ask the user. A job worth keeping: leave it and work in another tab. |
| Output ends early and the command "keeps running" | `run` stopped waiting (`within`), the command did not stop. | `ctx wait until "<tab>" is idle within 10 minutes`, then `ctx read last command in "<tab>"`. Don't start it again. |
| Relative paths hit the wrong folder | The tab keeps the folder it was left in; it doesn't follow your idea of the working directory. | Use absolute paths, or start the command with `cd /abs/path &&`. |
| `Operation not permitted` on a file, or ssh says `no such identity … Operation not permitted` then `Permission denied (publickey)` | Your agent runs in a sandbox (`ctx list agents` shows what it may read and write). The host or file is fine. | Don't retry variations. Tell the user which path was refused. |
| `… belongs to …` or refusal to type into a tab | Agents drive only their own tabs; most may read the user's. | Read it, and ask the user to act in their tab or hand it over. |
| `Names can use letters, digits, spaces, - and _` | Tab names can't contain `.` (or quotes). | `Pytest - auth_test` not `auth_test.py`. |
| `in 417` not understood | A bare number isn't a tab. | `in tab 417`. |
| `run` refuses: no shell-integration marks | The tab is inside ssh to a host ctx can't mark, or a non-zsh shell. | `ctx type "<cmd>" into "<tab>" then press enter`, then wait (see `persistent-terminals`). |

## Auto mode and ctx

The auto-mode classifier sees your tool calls as written.

- **"a hook changed this call's input"**: the hook rewrote the call, so the review didn't match what would run. Issue the call again once, as that message says. If it fails again, tell the user that the ctx hook and auto mode don't fit together for that call, and go on with other work.
- **Prefer single ctx sentences for ctx actions.** One sentence per Bash call, no `2>/dev/null`, no `; tail …`. It isn't rewritten, so what is reviewed is what runs, and it's easier for the user to read.
- **Never type or press keys into a tab running an agent**: your own session (the tab whose busy program is `claude`) or another agent's. That is an agent driving its own oversight and is blocked as `Tmux Self Drive`. Drive only tabs whose program you started for the task: a REPL, debugger, server, ssh.
- **A denial is final for that action.** Don't re-run a denied command through `ctx run`, `ctx type`, another tab, or a script. Say what was blocked and why it was needed. If the user thinks the denials are wrong, `integrations/ctx-skills/README.md` in the ctx repo explains how they can describe ctx in their own auto-mode settings.

## Long-running work goes in its own named tab

Prefer this over `run_in_background`, `nohup` or `&`. A tab survives the end of your session, the user can watch it in CTX.app, press ctrl-c and re-run a variant, and the next agent finds its history.

```
ctx open tab named "Dev server - site" at ~/code/site
ctx run in "Dev server - site" "bun run dev" without waiting
ctx wait until "Dev server - site" shows "Local:" within 2 minutes
ctx read last 40 lines in "Dev server - site"
```

Builds, test suites, training runs, crawlers, `gh run watch`, `tail -f`: same pattern. Check back with `ctx wait until "<tab>" is idle within 30 minutes` instead of `sleep`. For something that never goes idle (a server), wait on text or on quiet: `ctx wait until "<tab>" is quiet for 5 seconds`.

## Look before you redo

The record is the shared memory of everyone who used this machine's terminals.

- `ctx list tabs`: a server, REPL or ssh session someone already has open. Reuse it instead of starting a second copy.
- `ctx find "<command or error>" at <project folder> since 7 days ago`: earlier runs, by anyone, open or closed tabs.
- `ctx read last 10 commands in "<tab>"`: what was run, exit statuses, times. Add `up to 40 lines` to cap each output.
- `ctx list closed tabs`, then `ctx reopen tab 40`: same name, folder and environment.
- `ctx ask "<question>" about tab 212`: an answer with the exact supporting lines, when reading would cost many tokens.

Cite what you found as `tab 212, command 14`, so the user and later agents can open it.

## Reading without flooding your context

`ctx read last command in "<tab>"`, `ctx read last 50 lines in "<tab>"`, `ctx find "…" in "<tab>"`. `ctx run` prints at most 400 lines by default; add `up to 60 lines`. Never `read everything` on a busy tab. Add `as json` when you will parse the result.

## Arranging things for the human

The user sees tabs in CTX.app. Make the layout readable:

- **Name every tab you open** for what runs in it: `Tests - auth`, `ssh slopslinger`, `Python - sales df`, `Dev server - site`. Rename tabs you reuse: `ctx rename "x" to "Dev server - site"`.
- **One window per task** when there are several tabs: `ctx open window at ~/code/site named "Auth refactor"`, then `ctx open tab in window "Auth refactor" named "…"`. `ctx move "<tab>" to window "Auth refactor"` tidies up.
- **`ctx show "<tab>"`** brings a tab to the front and takes the user's focus. Use it only when you want them to look or act: a password prompt, a 2FA code, a confirmation only they can give, a result worth seeing live. Say in your reply what you want them to do there.
- **Hand-offs**: give the tab's name and number. Terminal users can `ctx attach to tab N` (ctrl-] detaches).
- **Never type a password or secret** yourself. Ask the user to type it in the tab, then `ctx wait until "<tab>" shows "<text that only appears after login>"`.
- Close tabs you opened only for a one-off (`ctx close "<tab>" without asking`). Leave long-lived ones (servers, REPLs with data loaded, ssh sessions) open and mention them in your summary.
- Leave tabs you didn't open alone unless asked.

For REPLs, debuggers, ssh, adb, database shells and sending several commands at once, use the `persistent-terminals` skill.

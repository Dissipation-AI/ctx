# When the shell seems broken

| What you see | What it means | What to do |
| --- | --- | --- |
| `Tab N "Claude …" is running "<cmd>"; open another tab` | Your session tab is still busy: an earlier background call, a call that outlived its timeout, or a program waiting for input (password, `[y/N]`, pager). Every later hooked call gets this until it is free. | Free it yourself (see "Keep your session tab free"): `ctx read last 30 lines in tab N`, then `ctx press ctrl-c in tab N`, or answer a safe prompt with `ctx type "n" into tab N then press enter`. A password: ask the user. A job worth keeping: leave it and work in another tab. |
| Output ends early and the command "keeps running" | `run` stopped waiting (`within`), the command did not stop. | `ctx wait until "<tab>" is idle within 10 minutes`, then `ctx read last command in "<tab>"`. Don't start it again. |
| Relative paths hit the wrong folder | The tab keeps the folder it was left in; it doesn't follow your idea of the working directory. | Use absolute paths, or start the command with `cd /abs/path &&`. |
| `Operation not permitted` on a file, or ssh says `no such identity … Operation not permitted` then `Permission denied (publickey)` | Your agent runs in a sandbox (`ctx list agents` shows what it may read and write). The host or file is fine. | Don't retry variations. Tell the user which path was refused. |
| `… belongs to …` or refusal to type into a tab | Agents drive only their own tabs; most may read the user's. | Read it, and ask the user to act in their tab or hand it over. |
| `Names can use letters, digits, spaces, - and _` | Tab names can't contain `.` (or quotes). | `Pytest - auth_test` not `auth_test.py`. |
| `in 417` not understood | A bare number isn't a tab. | `in tab 417`. |
| `ctx --help` or `ctx run --help` fails | ctx takes sentences, not flags. | `ctx help`, `ctx help run`. |
| `run` refuses: no shell-integration marks | The tab is inside ssh to a host ctx can't mark, or a non-zsh shell. | `ctx type "<cmd>" into "<tab>" then press enter`, then wait (see `interactive-programs`). |

## Freeing a stuck session tab

The session tab runs one thing at a time. Use it for commands that finish quickly and that you wait for: reads, edits, short builds and tests. Anything that runs long, waits for input, or that you want to keep going while you work goes in **another tab**: `ctx open tab named "<what>"`, then `ctx run in "<what>" "…" without waiting`. That includes anything that might prompt (`ssh`, `gcloud`, `sudo`, installers).

A recent ctx hook does part of this for you, so check `ctx list tabs` to see where a call went:

- A Bash call with `run_in_background` runs in its own copy of the session tab, named for the command: `Claude 4db0312f - bun run dev`. The same command later reuses that tab once it is idle.
- A call that finds the session tab busy runs in `Claude 4db0312f 2` (then `3`…), a copy with the same folder and exported variables.

`ctx open tab like "<tab>"` makes such a copy yourself: same folder, exported variables and history, a fresh shell. Unexported variables and functions don't carry over, so `export` what later tabs should see.

If your session tab gets stuck, free it yourself. Don't ask the user to. Single ctx sentences aren't routed through the busy tab:

```
ctx read last 30 lines in "Claude 4db0312f"
ctx press ctrl-c in "Claude 4db0312f"
```

Read first. If it is a job worth keeping, leave it and open another tab. If it is a prompt or a hang, press ctrl-c (twice if needed; `ctx press ctrl-d` or `q` for some programs), then re-run the long part in its own tab.

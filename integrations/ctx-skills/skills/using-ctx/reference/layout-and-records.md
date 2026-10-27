# Arranging tabs for the human, and using the record

## Long-running work goes in its own named tab

Prefer this over Claude Code's background shells and `nohup`. A tab survives the end of your session, the user can watch it in CTX.app, press ctrl-c and re-run a variant, and the next agent finds its history.

```
ctx open tab named "Dev server - site" at ~/code/site
ctx run in "Dev server - site" "bun run dev" without waiting
ctx wait until "Dev server - site" shows "Local:" within 2 minutes
ctx read last 40 lines in "Dev server - site"
```

Builds, test suites, training runs, crawlers, `gh run watch`, `tail -f`: same pattern. `&`, `jobs` and `wait` inside a tab are fine for a batch of short parallel jobs; the `interactive-programs` skill covers job control. Check back with `ctx wait until "<tab>" is idle within 30 minutes` instead of `sleep`. For something that never goes idle (a server), wait on text or on quiet: `ctx wait until "<tab>" is quiet for 5 seconds`.

## Look before you redo

The record is the shared memory of everyone who used this machine's terminals.

- `ctx list tabs`: a server, REPL or ssh session someone already has open. Reuse it instead of starting a second copy.
- `ctx find "<command or error>" at <project folder> since 7 days ago`: earlier runs, by anyone, open or closed tabs.
- `ctx read last 10 commands in "<tab>"`: what was run, exit statuses, times. Add `up to 40 lines` to cap each output.
- `ctx list closed tabs`, then `ctx reopen tab 40`: same name, folder and environment.
- `ctx ask "<question>" about tab 212`: an answer with the exact supporting lines, when reading would cost many tokens.

Cite what you found as `tab 212, command 14`, so the user and later agents can open it.

## Reading without flooding your context

Output is capped twice. `ctx run` prints at most 400 lines (the first and last lines, with a note saying what it left out); `up to 60 lines` lowers that. Then Claude Code gives you about 30,000 characters inline; past that you get a file path and a 2,000-character preview, and a failing command gets a ~10,000-character head-and-tail excerpt with no file. Neither cap is a reason to print a lot.

- Send big output to a file and query it: `bun test > $S/test.log 2>&1; tail -30 $S/test.log`, then `grep -n FAIL`, `sed -n 120,160p`, `awk`. The file stays for the next question; re-running doesn't.
- Pagers are off in tabs agents open (`PAGER=cat`, `GIT_PAGER=cat` and friends). In a tab the user opened, a pager can still take the screen: `ctx read "<tab>"` shows `:` at the bottom, `ctx press q in "<tab>"` leaves. Use `git --no-pager` there.
- Don't use `less`, `vim` or other full-screen viewers and editors to read or change files; `grep`, `sed -n`, `awk`, `head` and `tail` answer in one call. If a command opens an editor (`git commit` without `-m`, `crontab -e`, `git rebase -i`), give it the text with flags or `GIT_EDITOR=true`; if one is already open, `ctx press escape in "<tab>"` then `ctx type ':q!' into "<tab>" then press enter`.
- From the record: `ctx read last command in "<tab>"`, `ctx read last 50 lines in "<tab>"`, `ctx find "…" in "<tab>"`. Never `read everything` on a busy tab. Add `as json` when you will parse the result.

## Arranging things for the human

The user sees tabs in CTX.app. Make the layout readable:

- **Name every tab you open** for what runs in it: `Tests - auth`, `ssh build-box`, `Python - sales df`, `Dev server - site`. Rename tabs you reuse: `ctx rename "x" to "Dev server - site"`.
- **One window per task** when there are several tabs: `ctx open window at ~/code/site named "Auth refactor"`, then `ctx open tab in window "Auth refactor" named "…"`. `ctx move "<tab>" to window "Auth refactor"` tidies up.
- **`ctx show "<tab>"`** brings a tab to the front and takes the user's focus. Use it only when you want them to look or act: a password prompt, a 2FA code, a confirmation only they can give, a result worth seeing live. Say in your reply what you want them to do there.
- **Hand-offs**: give the tab's name and number. Terminal users can `ctx attach to tab N` (ctrl-] detaches).
- **Never type a password or secret** yourself. Ask the user to type it in the tab, then `ctx wait until "<tab>" shows "<text that only appears after login>"`.
- Close tabs you opened only for a one-off (`ctx close "<tab>" without asking`). Leave long-lived ones (servers, REPLs with data loaded, ssh sessions) open and mention them in your summary.
- Leave tabs you didn't open alone unless asked.

For REPLs, debuggers, ssh, adb, database shells and sending several commands at once, use the `interactive-programs` skill.

# Claude Code auto mode and ctx

The auto-mode classifier sees your tool calls as written.

- **"a hook changed this call's input"**: the hook rewrote the call, so the review didn't match what would run. Issue the call again once, as that message says. If it fails again, tell the user that the ctx hook and auto mode don't fit together for that call, and go on with other work.
- **Chain freely.** `cd /w/site && bun test 2>&1 | tail -30`, or `ctx type '…' into "Python - sales" then press enter && ctx wait until "Python - sales" shows "DONE-2" within 1 minutes && ctx read last 20 lines in "Python - sales"`, saves a round trip each time. Two kinds of call stay on their own, as one plain ctx sentence: anything aimed at your own session tab (`ctx press ctrl-c in "Claude 4db0312f"`), and a call you are retrying after the hook message above.
- **Never type or press keys into a tab running an agent**: your own session (the tab whose busy program is `claude`) or another agent's. That is an agent driving its own oversight and is blocked as `Tmux Self Drive`. Drive only tabs whose program you started for the task: a REPL, debugger, server, ssh.
- **A denial is final for that action.** Don't re-run a denied command through `ctx run`, `ctx type`, another tab, or a script. Say what was blocked and why it was needed. If the user thinks the denials are wrong, `integrations/ctx-skills/README.md` in the ctx repo explains how they can describe ctx in their own auto-mode settings.

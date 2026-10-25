# ctx skills

Two [Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview) for agents that work in ctx:

- **`using-ctx`**: how ctx may be wired into the agent's shell, what the messages that make the shell look broken mean (a busy session tab, a command that outlived `within`, the agent sandbox, auto-mode denials), why long jobs go in their own named tab, how to find what earlier agents and you already ran, and how to lay out tabs and windows so they're easy for you to follow in CTX.app.
- **`persistent-terminals`**: when a persistent terminal beats a one-shot command, setting variables once instead of repeating long paths, sending several commands to a REPL or debugger in one call, waiting without `sleep` or races, working with several tabs and passing context between them, and handing a tab to you for passwords. Notes for Python/pdb, gdb/lldb, ssh, adb and database shells are loaded only when needed.

They don't need the Bash hook; they help with or without it.

## Install

Claude Code:

```sh
claude plugin install ctx-skills --marketplace Dissipation-AI/ctx
```

The skills show up as `ctx-skills:using-ctx` and `ctx-skills:persistent-terminals`, and load on their own when a task calls for them. `claude plugin update ctx-skills@ctx` picks up new versions.

If you installed ctx's hook with `ctx install hooks for claude`, you already have a marketplace named `ctx` (ctx's own copy of `integrations/`). With a ctx that includes these skills, `claude plugin install ctx-skills@ctx` installs them from it.

Other agents that read `SKILL.md` folders (Codex, Cursor, Gemini CLI, opencode and others) can use the [skills CLI](https://github.com/vercel-labs/skills), which finds every `SKILL.md` in the repository:

```sh
bunx skills add Dissipation-AI/ctx
```

or copy `skills/using-ctx` and `skills/persistent-terminals` into the agent's skills folder.

## Auto mode

Claude Code's auto-mode classifier blocks an agent from sending keystrokes to its own interface (`Tmux Self Drive`), and it can't review a Bash call that the ctx hook rewrote. The skills tell agents to keep calls aimed at their own session tab to single, plain `ctx` sentences, never to type into a tab running an agent, and to stop and tell you when a call is denied rather than try another route.

If the classifier still blocks ordinary ctx work (typing into a REPL tab the agent opened), describe ctx to it in your own settings. Add a line like this to `autoMode.environment` in `~/.claude/settings.json`:

```
**Terminal**: shell commands run in ctx tabs (https://github.com/Dissipation-AI/ctx). `ctx type`, `ctx press` and `ctx run` drive the program in a named tab (a REPL, debugger, ssh session or server); they don't reach Claude Code's own interface unless that tab's program is `claude`. A tab named `Claude <id>` is the session's shell, not its interface.
```

`claude auto-mode config` shows what is in effect, and `claude auto-mode defaults` prints the built-in list to start from. Your entry is your own judgment about your machine; the skills never edit these settings.

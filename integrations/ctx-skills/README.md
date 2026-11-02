# ctx skills

Two [Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview) for agents that work in ctx:

- **`using-ctx`**: how ctx may be wired into the agent's shell, what the messages that make the shell look broken mean (a busy session tab, a command that outlived `within`, the agent sandbox, auto-mode denials), why long jobs go in their own named tab, how to find what earlier agents and you already ran, and how to lay out tabs and windows so they're easy for you to follow in CTX.app.
- **`interactive-programs`**: running a program that needs a terminal in a ctx tab and typing into it: consoles and CLIs that refuse to run without a TTY, setup wizards (every known answer typed ahead in one call), REPLs, debuggers, ssh, adb and database shells, one call per round, waiting without `sleep`, several tabs at once, and handing a tab to you for passwords. Notes for Python/pdb, gdb/lldb, ssh, adb and database shells are loaded only when needed.

They don't need the Bash hook; they help with or without it.

## Install

With ctx installed, `ctx install skills for <agent>` (claude, antigravity, codex, cursor, pi or opencode; add `at <folder>` for one project) installs them through the agent's plugin registry, or copies them into its skills folder where it has none; `ctx remove skills for <agent>` undoes it. `ctx install hooks for <agent>` offers them when run in a terminal. The table in [ctx's README](../../README.md#skills-teaching-agents-to-use-ctx) lists where each one goes.

This folder is a plugin for several agents at once: `.claude-plugin/plugin.json` (Claude Code), `plugin.json` (Antigravity; a project's `.agents/plugins.json` loads only native plugins), `.codex-plugin/plugin.json` (Codex, listed in `../.agents/plugins/marketplace.json`) and `package.json` (a pi package).

Without ctx, in Claude Code:

```sh
claude plugin install ctx-skills --marketplace Dissipation-AI/ctx
```

The skills show up as `ctx-skills:using-ctx` and `ctx-skills:interactive-programs`. `claude plugin update ctx-skills@ctx` picks up new versions. Other agents that read `SKILL.md` folders can copy `skills/using-ctx` and `skills/interactive-programs` into their skills folder.

Checked on 2026-11-02 by installing into a throwaway home: Claude Code (`claude plugin list`), Antigravity (`agy plugin list`, and a project's plugins.json), Codex 0.142.2 (`codex debug prompt-input` lists both skills, as a plugin and from a project's `.agents/skills`) and opencode (`opencode debug skill`, user and project folders). Cursor's folders and pi's package format are from their docs ([Cursor](https://cursor.com/docs/skills), [pi](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/packages.md)) and not yet run.

## Auto mode

Claude Code's auto-mode classifier blocks an agent from sending keystrokes to its own interface (`Tmux Self Drive`), and it can't review a Bash call that the ctx hook rewrote. The skills tell agents to keep calls aimed at their own session tab to single, plain `ctx` sentences, never to type into a tab running an agent, and to stop and tell you when a call is denied rather than try another route.

If the classifier still blocks ordinary ctx work (typing into a REPL tab the agent opened), describe ctx to it in your own settings. Add a line like this to `autoMode.environment` in `~/.claude/settings.json`:

```
**Terminal**: shell commands run in ctx tabs (https://github.com/Dissipation-AI/ctx). `ctx type`, `ctx press` and `ctx run` drive the program in a named tab (a REPL, debugger, ssh session or server); they don't reach Claude Code's own interface unless that tab's program is `claude`. A tab named `Claude <id>` is the session's shell, not its interface.
```

`claude auto-mode config` shows what is in effect, and `claude auto-mode defaults` prints the built-in list to start from. Your entry is your own judgment about your machine; the skills never edit these settings.

# Agents and their accounts

An agent is an OS account in the group `ctx-agents`. What it may read, write and drive is the OS's answer: file permissions and access lists, nothing ctx keeps on the side.

## Who you are

- `id -un` says which account you run as. In an agent's tab it's the agent's name, and `$CTX_AGENT` is set.
- Your home is the agent's home: your CLAUDE.md, AGENTS.md, your programs' settings and logins. `ctx-agent.toml` there belongs to the person; you can read it but not change it.
- Logins are yours to make: the first time `claude`, `codex` or `pi` runs as you, it asks you to log in, like any new user on the machine.

## What you can reach

- Your home, `$CTX_AGENTS/Shared/everyone`, and shared folders you're part of (`$CTX_AGENTS/Shared/a+b`).
- What the person shared with you: `ctx show agent "<you>"` lists it. Anything else is refused by the OS ("Permission denied"). Ask the person; they run `ctx allow agent "<you>" to read <path>`.
- You can share your own files with another agent: `ctx allow agent "<other>" to read <path in your home>`. Nobody can share what isn't theirs.
- `ctx make shared folder with agent "<other>"` makes a folder for the two of you (and the person).

## Tabs

- You see your own tabs. You see another agent's tabs if you can read its home, and drive them if you can write it. You never see the person's tabs.
- `ctx switch to agent "<name>"` opens a tab as an agent (the person, or an agent administering that one).

## Handing things to the person

`open <url or file>`, `xdg-open` and `pbcopy` in your tabs hand links, files and text to the person's session: a file opens from a read-only copy, and only documents and media open. `pbpaste` is refused: the person's clipboard isn't yours.

## Sentences

```
ctx show agent "<name>"                      what it may reach, its limits, its tabs
ctx list agents
ctx allow agent "<a>" to read|write <path>   share a file or folder you own
ctx forbid agent "<a>" to read <path>        take it back
ctx allow agent "<a>" to read|write agent "<b>"   see / administer b
```

Making, removing and limiting agents is the person's: `ctx make agent named "<name>"`, `ctx remove agent "<name>"`, `ctx limit agent "<name>" to 8 GB of memory`.

# ctx output schemas

_Generated from `ctx-protocol/src/docs.rs` and the types in `ctx-protocol`. Don't edit by hand._

Every sentence can answer `as json` or `as toon`; both carry exactly the same fields. The schema version is `1`
and comes first in every answer. Field names are whole words; times are ISO 8601 in UTC; a value that doesn't apply is `null`.

Errors have the same shape everywhere:

```json
{
  "schema": 1,
  "error": "no_such_tab",
  "message": "There is no tab 9. Open tabs are 41, 42 and 43."
}
```

`error` is one of: `not_a_valid_command`, `no_such_tab`, `no_such_window`, `not_allowed`, `busy`, `someone_typing`,
`needs_confirmation`, `time_limit`, `unsupported`, `failed`. A sentence that doesn't parse also carries `suggestion` and
`explanation`.

## `ctx list tabs`

Open windows and their tabs.

Fields: `schema` (number), `windows` (list), `windows[].number` (number), `windows[].name` (text), `windows[].tabs` (list), `windows[].tabs[].number` (number), `windows[].tabs[].name` (text), `windows[].tabs[].folder` (text), `windows[].tabs[].window` (number), `windows[].tabs[].opened_by` (text), `windows[].tabs[].owner` (text), `windows[].tabs[].title` (nullable), `windows[].tabs[].state` (text), `windows[].tabs[].running` (text), `windows[].tabs[].pid` (number), `windows[].tabs[].started` (text), `windows[].tabs[].unread_lines` (number), `windows[].tabs[].columns` (number), `windows[].tabs[].rows` (number), `windows[].tabs[].shell_integration` (boolean), `windows[].tabs[].permissions_of` (nullable), `windows[].tabs[].remote_host` (nullable), `caller` (nullable).

```json
{
  "schema": 1,
  "windows": [
    {
      "number": 2,
      "name": "Work",
      "tabs": [
        {
          "number": 43,
          "name": "Dev server",
          "folder": "/Users/c/code/site",
          "window": 2,
          "opened_by": "dev (claude, pid 5512)",
          "owner": "dev",
          "title": null,
          "state": "busy",
          "running": "npm run dev",
          "pid": 81234,
          "started": "2026-10-10T17:52:01Z",
          "unread_lines": 12,
          "columns": 120,
          "rows": 40,
          "shell_integration": true,
          "permissions_of": null,
          "remote_host": null
        }
      ]
    }
  ],
  "caller": null
}
```

```
schema: 1
windows[1]:
  - number: 2
    name: Work
    tabs[1]{number,name,folder,window,opened_by,owner,title,state,running,pid,started,unread_lines,columns,rows,shell_integration,permissions_of,remote_host}:
      43,Dev server,/Users/c/code/site,2,"dev (claude, pid 5512)",dev,null,busy,npm run dev,81234,"2026-10-10T17:52:01Z",12,120,40,true,null,null
caller: null
```

## `ctx list closed tabs`

Saved tabs that aren't running any more, newest first.

Fields: `schema` (number), `tabs` (list), `tabs[].number` (number), `tabs[].name` (text), `tabs[].folder` (text), `tabs[].opened_by` (text), `tabs[].opened_at` (text), `tabs[].closed_at` (text), `tabs[].ended` (text), `tabs[].commands` (number), `total` (number).

```json
{
  "schema": 1,
  "tabs": [
    {
      "number": 40,
      "name": "Sysadmin agent",
      "folder": "/Users/c/agents/sysadmin",
      "opened_by": "you",
      "opened_at": "2026-10-09T20:01:00Z",
      "closed_at": "2026-10-09T23:10:00Z",
      "ended": "closed",
      "commands": 31
    }
  ],
  "total": 1
}
```

```
schema: 1
tabs[1]{number,name,folder,opened_by,opened_at,closed_at,ended,commands}:
  40,Sysadmin agent,/Users/c/agents/sysadmin,you,"2026-10-09T20:01:00Z","2026-10-09T23:10:00Z",closed,31
total: 1
```

## `ctx list commands in "Dev server"`

A tab's commands without their output.

Fields: `schema` (number), `tab` (number), `tab_name` (text), `commands` (list), `commands[].number` (number), `commands[].command` (text), `commands[].where` (text), `commands[].ran_by` (text), `commands[].exit_status` (number), `commands[].started_at` (text), `commands[].finished_at` (text), `commands[].took_seconds` (number), `commands[].approximate` (boolean), `total` (number).

```json
{
  "schema": 1,
  "tab": 43,
  "tab_name": "Dev server",
  "commands": [
    {
      "number": 13,
      "command": "cargo build",
      "where": "c@macbook:~/code/site",
      "ran_by": "dev (claude, pid 5512)",
      "exit_status": 0,
      "started_at": "2026-10-10T17:52:01Z",
      "finished_at": "2026-10-10T17:52:40Z",
      "took_seconds": 39.0,
      "approximate": false
    }
  ],
  "total": 1
}
```

```
schema: 1
tab: 43
tab_name: Dev server
commands[1]{number,command,where,ran_by,exit_status,started_at,finished_at,took_seconds,approximate}:
  13,cargo build,"c@macbook:~/code/site","dev (claude, pid 5512)",0,"2026-10-10T17:52:01Z","2026-10-10T17:52:40Z",39.0,false
total: 1
```

## `ctx read last 3 commands in "Dev server"`

Commands with their output (also `read command N`). Output lines are numbered from 1 within each command: line 3 is `output[2]`.

Fields: `schema` (number), `tab` (number), `tab_name` (nullable), `commands` (list), `commands[].number` (number), `commands[].command` (text), `commands[].where` (text), `commands[].ran_by` (text), `commands[].exit_status` (number), `commands[].started_at` (text), `commands[].finished_at` (text), `commands[].took_seconds` (number), `commands[].approximate` (boolean), `commands[].full_screen` (boolean), `commands[].output` (list), `commands[].events` (list), `commands[].output_lines` (number), `commands[].note` (nullable).

```json
{
  "schema": 1,
  "tab": 212,
  "tab_name": null,
  "commands": [
    {
      "number": 13,
      "command": "cargo build",
      "where": "c@macbook:~/code/site",
      "ran_by": "dev (claude, pid 5512)",
      "exit_status": 0,
      "started_at": "2026-10-10T17:52:01Z",
      "finished_at": "2026-10-10T17:52:40Z",
      "took_seconds": 39.0,
      "approximate": false,
      "full_screen": false,
      "output": [
        "   Compiling site v0.1.0",
        "    Finished dev profile"
      ],
      "events": [],
      "output_lines": 2,
      "note": null
    },
    {
      "number": 14,
      "command": "cargo test -p auth",
      "where": "c@macbook:~/code/site",
      "ran_by": "you",
      "exit_status": 101,
      "started_at": "2026-10-10T17:52:01Z",
      "finished_at": "2026-10-10T17:52:40Z",
      "took_seconds": 39.0,
      "approximate": false,
      "full_screen": false,
      "output": [
        "test result: FAILED. 13 passed; 1 failed"
      ],
      "events": [],
      "output_lines": 1,
      "note": null
    }
  ]
}
```

```
schema: 1
tab: 212
tab_name: null
commands[2]:
  - number: 13
    command: cargo build
    where: "c@macbook:~/code/site"
    ran_by: "dev (claude, pid 5512)"
    exit_status: 0
    started_at: "2026-10-10T17:52:01Z"
    finished_at: "2026-10-10T17:52:40Z"
    took_seconds: 39.0
    approximate: false
    full_screen: false
    output[2]: "   Compiling site v0.1.0","    Finished dev profile"
    events[0]:
    output_lines: 2
    note: null
  - number: 14
    command: cargo test -p auth
    where: "c@macbook:~/code/site"
    ran_by: you
    exit_status: 101
    started_at: "2026-10-10T17:52:01Z"
    finished_at: "2026-10-10T17:52:40Z"
    took_seconds: 39.0
    approximate: false
    full_screen: false
    output[1]: "test result: FAILED. 13 passed; 1 failed"
    events[0]:
    output_lines: 1
    note: null
```

## `ctx read last 200 lines in "Dev server"`

Text: the screen, the last N lines, or everything.

Fields: `schema` (number), `tab` (number), `tab_name` (text), `lines` (list), `events` (list), `events[].line` (number), `events[].event` (text), `events[].at` (text), `events[].from` (text), `events[].to` (text), `total_lines` (number), `note` (text).

```json
{
  "schema": 1,
  "tab": 43,
  "tab_name": "Dev server",
  "lines": [
    "[renamed \"vite\" → \"Dev server\"]",
    "  VITE v5.4.2  ready in 312 ms",
    "  ➜  Local:   http://localhost:5173/"
  ],
  "events": [
    {
      "line": 1,
      "event": "renamed",
      "at": "2026-10-10T17:50:00Z",
      "from": "vite",
      "to": "Dev server"
    }
  ],
  "total_lines": 12503,
  "note": "showing the last 2 of 12,503 lines; ctx read everything in \"Dev server\" for all"
}
```

```
schema: 1
tab: 43
tab_name: Dev server
lines[3]: "[renamed \"vite\" → \"Dev server\"]","  VITE v5.4.2  ready in 312 ms","  ➜  Local:   http://localhost:5173/"
events[1]{line,event,at,from,to}:
  1,renamed,"2026-10-10T17:50:00Z",vite,Dev server
total_lines: 12503
note: "showing the last 2 of 12,503 lines; ctx read everything in \"Dev server\" for all"
```

## `ctx run in "Dev server" "npm test"`

What a command printed and its exit status. `still_running` is true after `without waiting`, or when a time limit ran out (exit status 3).

Fields: `schema` (number), `tab` (number), `tab_name` (text), `command_number` (number), `command` (text), `exit_status` (number), `still_running` (boolean), `took_seconds` (number), `output` (list), `events` (list), `output_lines` (number), `note` (nullable).

```json
{
  "schema": 1,
  "tab": 43,
  "tab_name": "Dev server",
  "command_number": 15,
  "command": "npm test",
  "exit_status": 0,
  "still_running": false,
  "took_seconds": 2.4,
  "output": [
    "Tests: 14 passed, 14 total"
  ],
  "events": [],
  "output_lines": 1,
  "note": null
}
```

```
schema: 1
tab: 43
tab_name: Dev server
command_number: 15
command: npm test
exit_status: 0
still_running: false
took_seconds: 2.4
output[1]: "Tests: 14 passed, 14 total"
events[0]:
output_lines: 1
note: null
```

## `ctx wait until "Dev server" shows "Listening on"`

What was waited for. `condition` is `idle`, `shows` or `quiet`.

Fields: `schema` (number), `tab` (number), `tab_name` (text), `condition` (text), `exit_status` (nullable), `line` (text), `waited_seconds` (number).

```json
{
  "schema": 1,
  "tab": 43,
  "tab_name": "Dev server",
  "condition": "shows",
  "exit_status": null,
  "line": "Listening on http://localhost:4000",
  "waited_seconds": 1.8
}
```

```
schema: 1
tab: 43
tab_name: Dev server
condition: shows
exit_status: null
line: "Listening on http://localhost:4000"
waited_seconds: 1.8
```

## `ctx find "deploy" by claude`

Matches in names, commands and output, newest first, with two lines of context.

Fields: `schema` (number), `text` (text), `hits` (list), `hits[].tab` (number), `hits[].tab_name` (text), `hits[].found_in` (text), `hits[].command` (number), `hits[].command_text` (text), `hits[].line` (number), `hits[].ran_by` (text), `hits[].at` (text), `hits[].text` (text), `hits[].before` (list), `hits[].after` (list), `total` (number), `note` (nullable).

```json
{
  "schema": 1,
  "text": "deploy",
  "hits": [
    {
      "tab": 212,
      "tab_name": "Claude - release",
      "found_in": "output",
      "command": 9,
      "command_text": "npm run deploy",
      "line": 4,
      "ran_by": "dev (claude, pid 5512)",
      "at": "2026-10-08T11:20:00Z",
      "text": "deploy finished in 41s",
      "before": [
        "uploading assets",
        "invalidating cache"
      ],
      "after": []
    }
  ],
  "total": 1,
  "note": null
}
```

```
schema: 1
text: deploy
hits[1]:
  - tab: 212
    tab_name: Claude - release
    found_in: output
    command: 9
    command_text: npm run deploy
    line: 4
    ran_by: "dev (claude, pid 5512)"
    at: "2026-10-08T11:20:00Z"
    text: deploy finished in 41s
    before[2]: uploading assets,invalidating cache
    after[0]:
total: 1
note: null
```

## `ctx list activity by claude since 1 hour ago`

The activity log, newest first. `client` is verified from the OS; `declared_name` is what the client claimed.

Fields: `schema` (number), `entries` (list), `entries[].at` (text), `entries[].client` (text), `entries[].program` (text), `entries[].pid` (number), `entries[].declared_name` (text), `entries[].sentence` (text), `entries[].tab` (number), `entries[].typed` (text), `entries[].typed_hidden` (boolean).

```json
{
  "schema": 1,
  "entries": [
    {
      "at": "2026-10-10T17:53:02Z",
      "client": "claude (pid 5512) in tab 2",
      "program": "claude",
      "pid": 5512,
      "declared_name": "Claude - auth refactor",
      "sentence": "ctx run in \"Dev server\" \"npm test\"",
      "tab": 43,
      "typed": "npm test",
      "typed_hidden": false
    }
  ]
}
```

```
schema: 1
entries[1]{at,client,program,pid,declared_name,sentence,tab,typed,typed_hidden}:
  "2026-10-10T17:53:02Z",claude (pid 5512) in tab 2,claude,5512,Claude - auth refactor,"ctx run in \"Dev server\" \"npm test\"",43,npm test,false
```

## `ctx status`

The server.

Fields: `schema` (number), `version` (text), `protocol` (number), `pid` (number), `uptime_seconds` (number), `socket` (text), `memory_bytes` (number), `saved_bytes` (number), `tabs` (list), `tabs[].number` (number), `tabs[].name` (text), `tabs[].memory_bytes` (number), `tabs[].scrollback_lines` (number).

```json
{
  "schema": 1,
  "version": "0.1.0",
  "protocol": 6,
  "pid": 81200,
  "uptime_seconds": 3600,
  "socket": "/var/folders/xx/T/ctx-501/server.sock",
  "memory_bytes": 21000000,
  "saved_bytes": 12000000,
  "tabs": [
    {
      "number": 43,
      "name": "Dev server",
      "memory_bytes": 1200000,
      "scrollback_lines": 1034
    }
  ]
}
```

```
schema: 1
version: "0.1.0"
protocol: 6
pid: 81200
uptime_seconds: 3600
socket: /var/folders/xx/T/ctx-501/server.sock
memory_bytes: 21000000
saved_bytes: 12000000
tabs[1]{number,name,memory_bytes,scrollback_lines}:
  43,Dev server,1200000,1034
```

## `ctx open tab named "Dev server"`

A new tab (or window, with `new_window`). `not_restored` lists variables kept out by `dont_save_env` when reopening.

Fields: `schema` (number), `tab` (number), `window` (number), `new_window` (boolean), `window_name` (nullable), `name` (text), `folder` (text), `not_restored` (list).

```json
{
  "schema": 1,
  "tab": 43,
  "window": 2,
  "new_window": false,
  "window_name": null,
  "name": "Dev server",
  "folder": "/Users/c/code/site",
  "not_restored": []
}
```

```
schema: 1
tab: 43
window: 2
new_window: false
window_name: null
name: Dev server
folder: /Users/c/code/site
not_restored[0]:
```

## `ctx close "Dev server"`

Done, in a sentence.

Fields: `schema` (number), `message` (text).

```json
{
  "schema": 1,
  "message": "Closed tab 43."
}
```

```
schema: 1
message: Closed tab 43.
```


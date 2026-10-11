# ctx events

_Generated from `ctx-store/src/record.rs`. Don't edit by hand._

Things that happen *to* a tab, as opposed to output from a program, are drawn as one dimmed line in
square brackets between output lines, included in `ctx read` and `ctx run` output, and kept in the saved
record as events (never as text, so a program can't fake one). The set is fixed:

| Event | Line | JSON |
|---|---|---|
| A tab was opened (by you or an agent). | `[opened 2026-10-10 17:52 by claude (pid 5512)]` | `{"event":"opened","at":"2026-10-10T17:52:01Z","by":"claude (pid 5512)"}` |
| A closed tab came back after a reboot. | `[reopened 2026-10-10 17:52 after reboot · last active 2026-10-10 16:52]` | `{"event":"reopened","at":"2026-10-10T17:52:01Z","reason":"reboot","last_active":"2026-10-10T16:52:01Z"}` |
| A closed tab was reopened. | `[reopened 2026-10-10 17:52 · last active 2026-10-10 16:52]` | `{"event":"reopened","at":"2026-10-10T17:52:01Z","reason":null,"last_active":"2026-10-10T16:52:01Z"}` |
| Open tabs came back after the server restarted. | `[reopened 2026-10-10 17:52 after server restart]` | `{"event":"reopened","at":"2026-10-10T17:52:01Z","reason":"server restart","last_active":"2026-10-10T17:52:01Z"}` |
| The tab was renamed. | `[renamed "auth" → "Claude - auth refactor"]` | `{"event":"renamed","at":"2026-10-10T17:52:01Z","from":"auth","to":"Claude - auth refactor"}` |
| The tab got its first name. | `[named "Dev server"]` | `{"event":"renamed","at":"2026-10-10T17:52:01Z","from":null,"to":"Dev server"}` |
| The tab moved to another window. | `[moved to window 2 "Work"]` | `{"event":"moved","at":"2026-10-10T17:52:01Z","window":2,"window_name":"Work"}` |
| An interactive ssh session started. | `[ssh to deploy@web-1 started 17:52]` | `{"event":"ssh_started","at":"2026-10-10T17:52:01Z","target":"deploy@web-1"}` |
| It ended (`closed` or `connection lost`). | `[ssh to deploy@web-1 ended 17:52 · connection lost]` | `{"event":"ssh_ended","at":"2026-10-10T17:52:01Z","target":"deploy@web-1","how":"connection lost"}` |
| A program tried to erase the scrollback; ctx kept it. | `[program cleared the screen — earlier output is kept in the saved record]` | `{"event":"screen_cleared","at":"2026-10-10T17:52:01Z"}` |
| The tab was closed. | `[closed 2026-10-10 17:52]` | `{"event":"closed","at":"2026-10-10T17:52:01Z"}` |

Times in lines are local; the examples use UTC.

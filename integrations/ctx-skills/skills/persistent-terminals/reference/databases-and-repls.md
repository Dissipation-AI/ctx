# Database shells and other REPLs

Everything here takes a one-shot form first (`psql -c`, `sqlite3 db 'sql'`, `mysql -e`, `redis-cli CMD`, `node -e`). Use a tab when you'll run many queries, need a transaction open, or the connection needs a password typed once.

## sqlite3

```
ctx run in "SQL - app db" "sqlite3 -box app.db" without waiting
ctx wait until "SQL - app db" is quiet for 1 seconds
ctx type 'select count(*) from users;
select * from users order by created_at desc limit 5;
select "DONE" || "-1";' into "SQL - app db" then press enter
ctx wait until "SQL - app db" shows "DONE-1" within 30 seconds
```

- Dot-commands (`.mode`, `.schema`, `.headers`) go in their own `type`. In a fast batch they were merged into the preceding SQL and failed.
- Set the output mode at launch (`-box`, `-csv`, `-json`) to avoid needing dot-commands at all.

## psql

```
ctx run in "SQL - prod replica" "psql -X -P pager=off \"$DATABASE_URL\"" without waiting
```

- `-X` skips `~/.psqlrc`, and `-P pager=off` stops `less` from taking the tab. `\x auto` for wide rows.
- Backslash commands (`\d users`, `\timing on`) in their own `type`.
- Marker: `select 'DONE' || '-1';`.
- In an open transaction, a failed statement aborts the transaction (`current transaction is aborted`): `rollback;` before continuing. `\q` with a transaction open doesn't prompt, it rolls back.
- Never run writes against production without the user saying so.

## mysql / mariadb

`mysql --pager=cat -t …`, marker `select concat('DONE','-1');`. Password: let the user type it at the prompt rather than putting `-p<password>` on the command line, which lands in the record.

## redis-cli

`redis-cli -h host --no-raw` in a tab; each line is a command. Marker: `ECHO DONE-1` echoes the same text you typed, so use `EVAL "return 'DONE'..'-1'" 0` or wait until quiet instead.

## node

```
ctx run in "Node - api" "node" without waiting
ctx type 'const db = require("./db")
const rows = await db.query("select 1 as ok")
console.log(rows, "DONE" + "-1")' into "Node - api" then press enter
```

Top-level `await` works in the node REPL. `.editor` mode takes multi-line code until `ctrl-d`.

## Other prompt-driven programs

The same loop works for `ghci`, `irb`/`pry`, `R`, `julia`, `octave`, `mongosh`, `lua`, `swift repl`, `ocaml`/`utop`, `erl`/`iex`: start in its own tab, wait until quiet, `type` a batch ending in a computed marker, wait for the marker, read the last lines. Turn off colours and fancy line editing where the program allows (`R --no-readline`, `julia --color=no`; check `--help` for others).

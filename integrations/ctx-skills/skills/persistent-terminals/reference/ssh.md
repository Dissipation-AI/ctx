# ssh and remote shells

## One-shot or persistent

`ssh host 'cmd'` for a single answer. A persistent ssh tab when you will run many commands, need a remote environment (venv, `cd`, variables) to stick, or the login needs a human (password, 2FA, passphrase).

## Opening an ssh tab

```
ctx open tab named "ssh slopslinger"
ctx run in "ssh slopslinger" "ssh slopslinger" without waiting
ctx wait until "ssh slopslinger" is quiet for 3 seconds
ctx read "ssh slopslinger"
```

Then read the screen and act on it:

- A shell prompt from the remote host: you're in.
- `password:`, `Verification code:`, `Enter passphrase`: the user's job. `ctx show "ssh slopslinger"`, ask them to log in, then `ctx wait until "ssh slopslinger" shows "<remote prompt text>" within 5 minutes`.
- `Are you sure you want to continue connecting (yes/no)?`: a new host key. Ask the user unless they told you this host is expected.
- `no such identity: … Operation not permitted` followed by `Permission denied (publickey)`: your agent's sandbox can't read the key file (`ctx list agents`). The host is fine. Tell the user; don't try other keys or options.

## Running commands on the remote side

With ctx's remote shell integration (on by default for interactive `ssh host` to a bash or zsh login shell), `ctx run`, `ctx wait until … is idle` and `ctx read last command` work in the ssh tab like a local one:

```
ctx run in "ssh slopslinger" "nvidia-smi --query-gpu=index,name,memory.used --format=csv"
```

If `run` refuses for lack of shell-integration marks (fish, plain `sh`, busybox, routers, or the host is excluded in ctx's settings), use `type` with a marker:

```
ctx type 'df -h /; echo DONE-$((40+2))' into "ssh router" then press enter
ctx wait until "ssh router" shows "DONE-42" within 30 seconds
```

## Long jobs on the remote

A job that must outlive the connection still needs `tmux` or `nohup` on the remote. Start it there, then come back with `ssh host 'tmux capture-pane -p -t job -S -50'`. Commands inside a remote tmux aren't recorded one by one by ctx, so prefer one ssh tab per remote job when the connection is stable.

## sudo

`sudo` prompts for a password: the user's job. Check first with `sudo -n true 2>&1`. If it says a password is required, ask the user to type it in the tab, or to run the command themselves.

## Several hosts

One tab per host, named `ssh <host>`. To compare hosts, run the same command in each and read each tab. Copy files between them with `scp`/`rsync` from a local tab, not by printing file contents into your context.

## Disconnects

`Connection to … closed` or a frozen tab: `ctx press enter` then `~.` (the ssh escape, typed as `ctx type "~." into "<tab>"` right after an Enter) ends a dead session, returning the tab to the local shell. A reopened ssh tab comes back with the `ssh` command filled in at the prompt, not yet run.

# Keeping Your System Awake

nightshift runs a loop that can take hours: one agent invocation per issue, repeated until a whole PRD is done. Most operating systems will suspend or sleep during that time if left unattended. This document describes how to prevent that on each platform.

## macOS

Wrap the nightshift command with `caffeinate -i`. This prevents the system from idle-sleeping for the duration of the process:

```bash
caffeinate -i nightshift --prd 10 --agent claude
```

`caffeinate` is built into macOS, so no install is required. When nightshift exits, the caffeine hold is released automatically.

## Linux

The simplest option is to run inside a **tmux** or **screen** session, which also keeps the process alive if your SSH connection drops:

```bash
# start a named session
tmux new-session -s nightshift

# inside the session, run normally
nightshift --prd 10 --agent claude

# detach with Ctrl-b d; nightshift keeps running in the background
# reattach later with:
tmux attach-session -t nightshift
```

Alternatively, use `systemd-inhibit` to block sleep at the system level:

```bash
systemd-inhibit --what=sleep --why="nightshift running" \
  nightshift --prd 10 --agent claude
```

## Windows

The recommended approach is to use **WSL (Windows Subsystem for Linux)** and follow the Linux instructions above.

If you are running natively on Windows, open **Settings → System → Power & sleep** and set "Sleep" to **Never** while nightshift is running, then restore your preferred setting afterwards.

Alternatively, run nightshift inside **Windows Terminal** and keep the session active. Closing the terminal window will terminate the process.
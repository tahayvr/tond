# Host terminal

The dashboard has a built-in terminal. It opens a shell inside a running app or database container, and, if you switch it on, a shell on the server itself as an unprivileged user. This guide explains how to use it, how to turn the host shell on and off, and what it can and can't do. You need an admin account.

## Open the terminal

Open the terminal pane with the terminal button in the dashboard footer, with **Ctrl+`**, or with **Toggle terminal** in the command palette (**Cmd/Ctrl+K**). Pick a target from the list:

| Target | What you get | Needs |
| --- | --- | --- |
| **Server** | A login shell on the host as the `tond` user | The host terminal switched on, and your password (and 2FA code) for every session |
| **App: `<name>`** | `sh` inside the app's running container | Admin |
| **Database: `<name>`** | `sh` inside the database's running container | Admin |

Only running apps and databases are listed. Press **Esc** to close the pane, or **Reconnect** to start a new session.

## Container terminals

A container terminal is `docker exec` into the running container, with the container's own user and tools. Use it to look at files, run a one-off migration or check what the app sees. Changes to the container's filesystem are lost on the next deploy, except in volumes. Container terminals are always available to admins, whether or not the host terminal is on.

For a database, the engine's client is already in the container, for example `psql` or `redis-cli`.

## The host shell

The host shell is off by default. It can only be switched on or off from the server itself, as root:

```sh
sudo tond settings host-terminal on
sudo tond settings host-terminal off
```

`tond settings` shows the current state. The command works over tond-server's root-only socket, so no dashboard session and no API token can turn the host shell on, even an admin's. While it is off, the **Server** target shows "The host terminal is off" and the command to run.

When it is on:

1. Pick **Server** in the terminal pane.
2. Tond asks you to **Open a shell on the server**: enter your **Password** and, if you use two-factor authentication, your **Authenticator code** (or a backup code).
3. Click **Open shell**.

Every host session asks again. There is no "remember me".

### Who the shell runs as

The shell runs `su -l tond`: a login shell as the `tond` user, never root. The installer creates this user with a locked password and a home directory, and removes it from the `docker`, `sudo`, `wheel` and `adm` groups on every install and update. The user is set by `TOND_TERMINAL_USER` in the `tond-server` systemd unit; tond-server refuses `root`.

That means the host shell can't:

- run `docker` commands or reach the Docker socket;
- use `sudo` or become root;
- read Tond's data directory, secrets or the local API socket.

It can look at the system (processes, disk usage, network), use tools installed for everyone, and keep files in `/home/tond`. For anything that needs root, use SSH.

Host shells run inside the tond-server service, so they share its memory and process limits.

## How sessions are protected

- **Admins only.** Every terminal session needs the `terminal:host` scope, which only the admin role has. API tokens can't open terminal sessions at all, whatever their scopes, because they can't do the password step.
- **Single-use tickets.** The dashboard first asks for a ticket, valid for 30 seconds and bound to your user, the target and the dashboard's address. The WebSocket must present it from the same origin, once.
- **Idle timeout.** A session closes after 15 minutes without input.
- **Audit.** Opening and closing every session is written to the [audit log](/concepts/audit-log) as `terminal_opened` and `terminal_closed`, with who opened which target. The log records sessions, not keystrokes.

## When to turn it off

Leave the host shell off unless you use it. Container terminals cover most debugging, and SSH covers the rest. If you turned it on for a task, turn it off afterwards:

```sh
sudo tond settings host-terminal off
```

See the [Security hardening checklist](/guides/hardening) for the other steps worth taking, and [Isolation](/security/isolation) for what separates the terminal user, apps and Tond itself.

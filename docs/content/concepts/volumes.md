# Volumes

A volume is a directory on the server that is mounted into an app's container, so data survives deploys, restarts and rollbacks. Without a volume, everything an app writes lives in its container and is gone when the next deploy replaces it. Use a volume for uploads, SQLite files or anything else the app must keep.

## Where volumes live

You never choose a path on the host. You give the volume a **name**, and Tond keeps it inside its own data directory:

```text
<data>/volumes/<app>/<volume name>
```

`<data>` is `TOND_DATA_DIR`, `/opt/tond/data` by default. An app named `blog` with a volume `uploads` mounted at `/app/uploads` stores its files in `/opt/tond/data/volumes/blog/uploads`.

This confinement matters because a bind mount is a door from a container into the host. If anyone could mount `/` or `/var/run/docker.sock`, deploying an app would give them root on the server. So Tond:

- accepts only a volume name, never a free host path;
- checks the path again right before it starts the container. It walks the path one directory at a time from the app's volume directory, refuses symbolic links and checks that the result is still inside. A container can't plant a symlink in its volume to escape on the next deploy.

## Adding a volume

Add volumes in the app's **Settings → Volumes**, or with the CLI:

```sh
tond apps volume add blog /app/uploads --name uploads
tond apps volume list blog
tond apps volume remove blog /app/uploads
```

| Field | Rules |
| --- | --- |
| Container path | Absolute, not `/`, at most 255 characters, no `.` or `..` segments, no `:` or `,` |
| Name | Letters, digits, `.`, `_` and `-`. It may not start with `.` and has at most 64 characters. If you leave it out, Tond derives it from the container path: `/var/lib/data` becomes `var_lib_data`. |

Each container path and each name can appear only once per app.

A new or removed volume takes effect on the **next deploy**. Removing a volume only unmounts it: the directory and its files stay on disk.

Volumes reach the host, so managing them needs an admin: the `apps:write` and `settings:write` scopes.

## Volumes and deploys

- A redeploy mounts the same directories into the new container, so the new version sees the old data straight away.
- Both containers briefly run side by side during a zero-downtime deploy, and both have the volume mounted. An app that holds an exclusive lock on a file, such as some SQLite setups, must cope with a second instance starting before the first one stops.
- A rollback restores the image, not the data. The volume keeps its current contents.
- When an app has volumes, every deploy starts a **pre-deploy snapshot** of them in the background (see below).

## Snapshots

Volumes are part of Tond's snapshots:

| Snapshot | Includes this app's volumes |
| --- | --- |
| Full snapshot | Yes, every app's volume directory |
| Pre-deploy snapshot | Yes, taken at the start of each deploy of an app with volumes |
| Database backup | No |

Snapshots copy regular files and directories only. Symbolic links, sockets and devices inside a volume are skipped, so a container can't make a snapshot read host files. Unchanged files are hard-linked to the previous snapshot, so repeated snapshots cost only the space of what changed.

To put an app's volume data back, restore a snapshot for that app. Tond stops the container, replaces `<data>/volumes/<app>` with the snapshot's copy and starts the container again. See [Snapshots and backups](/concepts/backups).

## Previews and cron jobs

- **Previews** get the same volume layout as their app, but with their own empty directories under `<data>/volumes/_preview/<slug>/`. They never see production data. The janitor removes these directories once the preview is gone.
- **Cron jobs** can have their own volumes, under `<data>/volumes/_cron/<job id>/`, or mount a volume of an existing app to work on its data.

## Docker Compose apps

Bind mounts in a compose file are rewritten to point inside the app's volume directory. `./data` becomes `<data>/volumes/<app>/data`. Absolute host paths outside it are refused when you save the file.

## Deleting an app

Deleting an app does not delete `<data>/volumes/<app>`. The data stays on disk until you remove it yourself. A new app created later with the same name mounts the same directory.

## Related

- [Apps](/concepts/apps)
- [Snapshots and backups](/concepts/backups)
- [Isolation](/security/isolation)
- [CLI reference](/reference/cli)

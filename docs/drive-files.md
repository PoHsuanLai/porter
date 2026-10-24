# A drive's files in Files: checking it by hand

With "Show in Files" on for an account, syncd shows the whole drive (OneDrive or My Drive) as a
folder: `~/Cloud/OneDrive`, `~/Cloud/Google Drive`, with the account's label after the name when
one provider has two. Every file is listed; a file's bytes come down when it is opened or asked
for, a chunk at a time, and stay in the cache until space is needed. This page is the check of
the real folder, which the tests never make: they run the drive in memory and mount nothing.

## What the computer needs

- The `fuse3` package (`fusermount3` on `PATH`, and `/dev/fuse`). porter does not link libfuse:
  it runs `fusermount3` as a process of its own. Without it the drive still lists, and Files says
  "Files can't show this drive on this computer."
- porter installed with `scripts/install.sh`, which puts in `porter-mountd` with its socket
  enabled for every user. syncd's unit cannot mount: `NoNewPrivileges=yes` refuses the setuid
  `fusermount3`, `PrivateDevices=yes` hides `/dev/fuse`, and its mount namespace of its own would
  hide the mount from the session. So syncd asks porter-mountd, a small user service outside that
  sandbox, which makes `~/Cloud/<name>`, runs `fusermount3` and hands syncd the open connection;
  syncd serves the files over it from inside its sandbox. porter-mountd's own unit goes without
  the settings that would break `fusermount3` (its header says which, and why).

## How the two fit

1. syncd connects to `$XDG_RUNTIME_DIR/porter/mountd.sock`; systemd, listening there for
   `porter-mountd.socket`, starts `porter-mountd.service`.
2. syncd asks to show "OneDrive" (a folder name, never a path), read-write (`rw`; `ro` for a
   drive shown read-only). porter-mountd checks the caller is the person, the name is one plain
   name and the folder is empty or new, clears a stale `fuse.porter` mount there, runs
   `fusermount3` (always with `nosuid,nodev,noatime`) and sends back the `/dev/fuse` fd.
3. syncd serves the drive over that fd. Turning the drive off asks porter-mountd to unmount it
   (`fusermount3 -u -z`, then the connection is ended), so syncd stops serving at once, even
   while a terminal sits in the folder.
4. With no porter-mountd listening (syncd started by hand, outside its unit), syncd mounts by
   itself through `fusermount3`.

## The check

1. Install (`scripts/install.sh`, then log in again or `systemctl --user daemon-reload` and
   `systemctl --user start porter-mountd.socket`). `systemctl --user status porter-mountd.socket`
   says it is listening; `systemctl --user status syncd.service` says syncd runs in its unit.
2. In Settings, turn on "Show in Files" for an account with a few files, a folder of photos and,
   for Google, a Doc and a Sheet.
3. `ls ~/Cloud/` lists the drive's folder; `ls -la ~/Cloud/OneDrive` (or `Google Drive`) lists
   the top of the drive with sizes and times as the drive gives them, before anything is
   downloaded. `du -sh ~/.cache/porter/drives/` is still about nothing.
4. Open a large file (a video) in a player and seek to the middle: it plays from there before the
   rest has come. `du` of the cache grows while it downloads.
5. A Google Doc shows as `Name.gdoc`, a Sheet as `Name.gsheet`: `cat` gives
   `{"url": ..., "doc_id": ...}`. Two files of one name show as `Name.ext` and `Name 2.ext`; a
   name with `/` in it shows it as `／`.
6. Writing is refused: `touch ~/Cloud/OneDrive/x` says "Read-only file system".
7. Turn the network off and open a file not yet downloaded: the open fails ("Network is down")
   and Files says "This file will download when you're connected." A file already here opens.
8. Keep Downloaded a folder in Files: everything under it comes down, and a file added to it on
   the web later comes down too after the next listing.
9. Remove Download a file: `du` of the cache shrinks; the file stays listed and comes back when
   opened. Remove Download a file inside a kept folder: refused, "This item is kept downloaded
   with its folder."
10. The limit: set Settings' "Downloaded Files Limit" (`storage.cache_limit`, bytes, `0` is
    automatic: a tenth of the disk, at least 1 GiB) below what is cached: the files opened
    longest ago go first; kept ones and open ones stay.
11. Turn "Show in Files" off: `~/Cloud/OneDrive` is no longer a mount (`mount | grep Cloud` says
    nothing). Do it again with a terminal sitting in the folder (`cd ~/Cloud/OneDrive`): it goes
    at once, syncd keeps answering, and the terminal's folder reads as gone. Turn it on and stop
    syncd (`systemctl --user stop syncd.service`): the mount goes with it. Kill it with
    `systemctl --user kill -s KILL syncd.service` and let it start again: the stale mount is
    cleared (`fusermount3 -u -z`) and the folder comes back.
12. A folder that is not porter's is left alone: stop syncd, put a file in an empty
    `~/Cloud/OneDrive` and start it: the drive says "The drive's folder in Cloud already has other
    files in it. Move them, then try again." and the file is still there.
13. syncd's sandbox is whole: `systemctl --user show syncd.service -p NoNewPrivileges -p
    PrivateDevices -p ProtectHome` says `yes`, `yes`, `read-only`. `journalctl --user -u
    porter-mountd` says why each refusal was made.

## Where things are

- The cache: `$XDG_CACHE_HOME/porter/drives/<account>/`: `content/` (one sparse file per item
  and version), `local/` (copies made here, waiting to upload), `index.json` (what is here, kept,
  and when each was last opened, written atomically with a `.bak`, after the content files it
  notes are synced) and `lock` (held by the one drive open over the directory). Removing the
  account removes it.
- The limit: `[storage] cache_limit = "<bytes>"` in `$XDG_CONFIG_HOME/porter/syncd/syncd.toml`,
  set by Settings (a `porter/syncd.toml` from before is read until the first save, and left).
- The helper: `/usr/libexec/quire/porter-mountd`, its units `porter-mountd.service` and
  `porter-mountd.socket`, its socket `$XDG_RUNTIME_DIR/porter/mountd.sock`.

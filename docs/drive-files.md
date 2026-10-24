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
- syncd running where it may mount: `fusermount3` is setuid, which the shipped unit's
  `NoNewPrivileges=yes` refuses, and the unit's `sandbox.conf` hides `/dev/fuse`
  (`PrivateDevices=yes`), keeps the home read-only and gives syncd a mount namespace of its own,
  where a mount would not be seen by the session. Until the unit allows it, check with syncd
  started by hand, outside the unit (stop `syncd.service` first).

## The check

1. Stop the unit, then start syncd by hand in a terminal:
   `systemctl --user stop syncd.service; /usr/libexec/quire/syncd`.
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
    nothing). Turn it on and stop syncd with `kill -TERM`: the mount goes with it. Kill it with
    `kill -9` and start it again: the stale mount is cleared (`fusermount3 -u`) and the folder
    comes back.

## Where things are

- The cache: `$XDG_CACHE_HOME/porter/drives/<account>/`: `content/` (one sparse file per item
  and version), `local/` (copies made here, waiting to upload), `index.json` (what is here, kept,
  and when each was last opened, written atomically with a `.bak`, after the content files it
  notes are synced) and `lock` (held by the one drive open over the directory). Removing the
  account removes it.
- The limit: `[storage] cache_limit = "<bytes>"` in `$XDG_CONFIG_HOME/porter/syncd.toml`, set by
  Settings.

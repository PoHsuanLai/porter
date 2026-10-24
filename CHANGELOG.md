# Changelog

## Unreleased

### A drive's files in Files, downloaded when opened

- With "Show in Files" on, syncd shows the whole drive as a folder (`~/Cloud/OneDrive`,
  `~/Cloud/Google Drive`), every file listed and its bytes downloaded when it is opened, the part
  read first. It needs the `fuse3` package; without it the drive still lists and Files says
  "Files can't show this drive on this computer." Nothing can be written there yet.
- `org.quire.Storage1` is served by syncd: `Drives`, `Items`, `Download`, `Keep`,
  `RemoveDownload`, `SaveCopy` (a Google document saved next to it as `.docx`, `.xlsx`, `.pptx` or
  `.png`), `SetPaused` and `Watch`, for the shell, Settings and Files.
- What is downloaded stays in `$XDG_CACHE_HOME/porter/drives/` until space is needed: past
  Settings' "Downloaded Files Limit" (`storage.cache_limit`, bytes, `0` for a tenth of the disk
  and at least 1 GiB) the files opened longest ago are removed; kept and open files never are.
- The folder works with syncd in its unit: a new user service, `porter-mountd` (started by
  `porter-mountd.socket`, installed and enabled by `scripts/install.sh`), runs `fusermount3` for
  it and hands back the open connection, so syncd keeps its whole sandbox. The helper answers only
  the person, puts a folder only in `~/Cloud`, refuses a folder with files in it, and never
  touches a mount that is not porter's. Run by hand, with no helper listening, syncd mounts by
  itself as before.
- Turning a drive off, or stopping syncd, takes its folder away at once, even while a terminal
  sits in it, and syncd keeps answering meanwhile. Two accounts of one provider with the same
  label get two folders ("OneDrive (Work)", "OneDrive (Work) 2").
- syncd's settings move to `$XDG_CONFIG_HOME/porter/syncd/syncd.toml`, the only folder of
  porter's configuration syncd may write; one at `porter/syncd.toml` is read until the first
  save, and left where it is.
- New crates `porter-drive` (the drive's files on demand, no file system attached),
  `porter-drive-fuse` (the folder, through `fusermount3`; libfuse is not linked) and
  `porter-mountd` (the helper; std and rustix only).
  `Replica::export` (porter-sync) has a default that refuses; Google Drive's uses `files.export`.

### Settings rows for drives and Photos

- "Keep Everything Downloaded" (`accounts.<id>.storage.keep`), offered while "Show in Files" is
  on: every file of the drive is downloaded and kept, also what is added to it later, and none is
  removed for space. Off goes back to the items kept one by one; nothing is removed then, and the
  rest may be removed past the limit again. Kept in `syncd.toml` (`[storage] keep_all`), so it
  holds after a restart. When the drive does not fit the free space, nothing more is downloaded,
  nothing is removed, and the row says "Not enough space to keep everything downloaded." The row
  is told of each time that changes, so the line comes and goes without reopening Settings. A
  free space that cannot be measured is never taken as short.
- Once an account shows in Files, its old "Keep this account's files on this computer" switch is
  hidden when it is off; while that mirror still runs the switch stays, so it can be turned off.
  Its folder is left where it is.
- The Photos row (`accounts.<id>.sync.photos`) is the only thing that turns Photos on: its grant
  starts the Photos datasets and taking it back stops them, keeping the photos. `SYNCD_PHOTOS` is
  gone, and `org.quire.Photos1.Picker` is served whenever syncd is built with `photos-picker`.
- porter-drive: `Drive::set_keep_all`, `keeps_all`, `short_of_room` and `watch_room`, the words
  `KEPT_WITH_DRIVE` and `NO_ROOM_FOR_ALL`, and `DriveConfig::free_space` (`FreeSpace`:
  measured, a figure, or unknown).

### Model lists say what a model takes in

- Each model row of `Inference1.Places` carries `takes` (text only, text and images, text and
  sound, or all three), worked out from the model's capabilities; a row with nothing to say leaves
  it out and an older reader ignores it. Hosted picker choices carry the same fact.
- A computer-use need is never given to a model that cannot read images, whatever its card
  claims. When no model can serve it, the session is refused with the new
  `InferRefusal::NoModelSeesScreen` (`porter_infer::NO_MODEL_SEES_SCREEN` words it); a model the
  person named that cannot read images is refused by name, `DeclinedBecause::CannotSeeScreen`
  (`porter_infer::cannot_see_screen`).

### Automatic checks that a model can use apps

- Before Automatic gives a request that offers tools to a model on this computer, inferd runs one
  small fixed check on it (made-up text, never the person's data) and keeps the result by model id
  in `tool-proofs.json`. A model that failed is skipped by Automatic for such requests; a model the
  person names is never skipped. When no model of the pool passes, the request is refused with the
  new `InferRefusal::NoModelUsesApps`. Hosted models are not checked; a turn that offered tools and
  ended unreadable marks its model.
- A tier with no model of its own is served by the person's pick for another tier of the slot
  (the asked tier, then Balanced, then the other). A slot with nothing set still refuses.

### Whoever uses a credential holds it

- An app no longer asks the account service for tokens, grants or ready-made connections over the
  bus. To continue as someone who is already signed in, it asks which identity (`Identities1`); to
  use the person's AI keys it asks the AI service for a permission (`Inference1.Keys`) and never
  sees a key. An app or service that needs tokens hosts its own account service in its own
  process.
- The coding-agent launcher now talks to the AI service, which keeps the AI keys: the same
  calls, with "grant" now "permit" (`request_agent_key`). Settings lists and withdraws these
  permissions from the AI service's own rows.
- The account service forgets, on its first start after the upgrade, the AI keys, permissions and
  switches it no longer holds. A copy of the old file is kept beside it (`registry.pre-split.json`).
  AI keys and sync are set up again, once.
- New: a shared library for the sheets the services show (`porter-sheets`), and a relay host for
  apps that run their own account service (`PorterRelays`).

## 0.1.0-beta.1 (2026-10-11)

The first release: the accounts work. Nothing here has yet run against the real services; it is
tested against stand-ins for them (FINDINGS.md says, per piece, what is still unchecked).

### Accounts

- One place for the accounts on a computer: mail, calendars, contacts, tasks, files, photos, AI
  services and Tailscale. An app asks for what it needs; the person decides, per app and per
  account, and can take it back.
- Passwords and keys are kept in the keyring (the Secret Service on Linux). Apps never receive a
  password. For mail and the web they get a connection that is already signed in; for an AI
  service a key that lasts as long as the permission does; otherwise a short-lived pass.
- Sign-in for Microsoft (Outlook, OneDrive, Calendar, Contacts, To Do), Google (Calendar,
  Contacts, Tasks, Drive's app folder, Photos, Gmail), Nextcloud, iCloud, Fastmail, Yahoo, GMX,
  any server that speaks mail, calendar or contacts protocols, and companies' API keys
  (Anthropic, OpenAI, Google AI, OpenRouter, Moonshot). Google needs a client of your own
  (docs/google.md). iCloud, Fastmail and Yahoo ask for an app password, as those services
  require.
- Adding an account that is already there says so and points at it, instead of failing. An app has
  one sign-in or consent sheet of each kind open at a time.
- Accounts for coding agents (Claude Code, Codex, Gemini and others): signed in and out
  through the program that runs them, a permission that lasts one session ("This session only"),
  and a scoped key handed to a spawned agent that is revoked when it ends. For a program that
  cannot be pointed at porter's AI service, the key goes to it as a private file (or a sealed
  memory handle), made when the program starts and removed when its permission ends.
- Tailscale as an account: add it like any other, and its state follows Tailscale. The shell and
  Settings can list the person's other computers on their network (`org.quire.Tailnet1`);
  nothing else asks Tailscale.
- Spaces: desktop-wide ones, and an app's own, so a permission can be given for a Space and not
  only for the whole desktop. An app's own Space belongs to that app.
- Finding a server from an address; asking the person to confirm what was found (a server found
  only through DNS outside the address's own domain is reviewed first); signing in again when a
  login expires; a seven-day reminder for a Google client still in testing; removal that also
  revokes at the service.
- The sign-in and consent sheets are drawn by the desktop; the Settings pages are fed by
  `org.quire.SettingsModule1`. A client id set in Settings is used by the next sign-in without a
  restart.
- An audit trail of what was released to whom.
- A damaged accounts file is not silently replaced: porter keeps the last good copy, says in plain
  words which files it looked at, and leaves them as they were.

### Sync (`syncd`)

- Calendar, contacts and tasks kept as local copies for Microsoft, Google, Nextcloud and
  other servers; a storage folder on Microsoft OneDrive, Google Drive's app folder and file servers such as Nextcloud;
  Photos (upload from a folder, the picker), off unless switched on.
- "Sync now": an app, Settings or a porter service can ask a dataset to sync at once instead of
  waiting for its next scheduled look (`org.quire.Sync1.SyncNow`). A sync already running finishes
  and one more follows it. A dataset the person paused says so and does not sync until it is
  resumed.
- A server that suddenly lists none, or most, of what a folder holds is not believed: nothing is
  deleted here, the person is told, and only their word lets it through (`ConfirmDiscard`).
  The shell is told of such a hold even for datasets it does not own.
- A sync that stalls on a server that stops answering gives up after a minute instead of waiting for
  ever, and a computer that wakes from sleep is noticed at the next short look (a minute at most), not
  at the end of a long wait. Removing an account waits for the sync in flight, so nothing is written after its files
  are deleted.

### AI (`inferd`)

- A broker for language, speech and computer-use models: on this computer (engines it starts, or
  ones you already run), on another of your computers, or a hosted one through an account, under
  the person's limits on what may leave the machine and what may be spent.
- Places: an app that runs the assistant can be told where it may run -- this computer, the
  person's own computers, signed-in cloud accounts -- and gets a plain refusal, with what would
  be needed, when none of them can serve the call. Settings can add and remove a computer.
- Your computers over Tailscale (lending, opt-in): with one switch ("Let my other computers use
  this computer's models") a computer lends the models that run on it to the person's other
  computers on their Tailscale network, and uses theirs. Off, nothing listens. The first time a
  computer asks, the person is asked; only the person's own computers can ask; a model from a
  cloud account is never lent. A computer can be named in any script.
- Choice scores: an app that asks a model to pick one of several options can also ask how likely
  each option was (the model's own odds, shares of 1000 across the options). Off unless asked for,
  and it never fails the call.
- An endpoint for outside coding agents' model calls.

### Libraries

Every part is a library with a thin program on top, so an app or a desktop can build with only
what it needs.

- `porter-client`: the app-facing API, with features for what it brings (`infer`, `in-process`,
  `engines` for apps that serve the OpenAI-compatible engines themselves, `lending` for the
  Settings switch above, `dbus`, `socket`). Typed calls for guests, computers, places, Spaces and
  the accounts removed; lists decode row by row, so one bad row does not hide the rest.
- `accountd`, `syncd` and `inferd` are libraries (`Config`, `Daemon::build`, `Daemon::run`); only the
  place a library reads the environment is `Config::from_env`.
- `porter-router` (which engine serves a call and the session machine), `porter-turns` (running a
  turn, structured replies, audit), `porter-tailnet` (who may ask this computer's models over the
  network), `porter-tailscale` (a client of Tailscale's local interface), `porter-daemon` (what the
  three daemons share), `porter-http`, `porter-fs` and `porter-families` (reads no environment and
  takes its clock from the caller) are crates of their own.
- `porter-dbus` has the bus names, the caller table and the sheet types as public modules.
- Each crate builds with only the features its users need; `scripts/check-features.sh` builds and
  lints each set a consumer uses.

### Packaging

- `scripts/install.sh` installs the three services, their systemd user units, the D-Bus
  activation files, the provider files and the caller table, under `PREFIX` and `DESTDIR`. The file
  that lets the AI service answer the person's other computers is put under `share/porter/` as
  data and never switched on; the Settings switch does that.
- README, licence texts (MIT or Apache-2.0), a Linux CI workflow (formatting, lints, licences, the
  crate boundaries, the feature sets, and the tests).

### Known gaps

- Nothing has been run against the real services yet: Microsoft, Google, Nextcloud, Tailscale and
  the hosted AI services are each tested against stand-ins that follow their documentation, and
  FINDINGS.md marks, per piece, what that leaves unchecked (the wording of real refusals, rate
  limits, the exact shape of real replies, a real Tailscale network with more than one computer).
- No shipped Microsoft client id yet (the owner registers the app); until one is set, Microsoft
  accounts say so. Google always needs your own.
- The tests that start a private session bus run only in the author's jail so far.
- Choice scores are a hint, not a calibrated probability, and depend on the model returning its
  first-token odds; where it does not, the answer carries none.
- `Broker::infer` in porter-infer and text-to-speech in inferd are not built. OpenRouter's browser
  sign-in is not built (its API key is).

# Changelog

## 0.1.0-beta.1 (not yet tagged)

The first release: the accounts work. Nothing here has yet run against the real services; it is
tested against stand-ins for them (FINDINGS.md says, per piece, what is still unchecked).

### Accounts

- One place for the accounts on a computer: mail, calendars, contacts, tasks, files, photos and
  AI services. An app asks for what it needs; the person decides, per app and per account, and can
  take it back.
- Passwords and keys are kept in the keyring (the Secret Service on Linux). Apps never receive a
  password. For mail and the web they get a connection that is already signed in; for an AI
  service a key that lasts as long as the permission does; otherwise a short-lived pass.
- Sign-in for Microsoft (Outlook, OneDrive, Calendar, Contacts, To Do), Google (Calendar,
  Contacts, Tasks, Drive's app folder, Photos, Gmail), Nextcloud, iCloud, Fastmail, Yahoo, GMX,
  any server that speaks mail, calendar or contacts protocols, and companies' API keys
  (Anthropic, OpenAI, Google AI, OpenRouter, Moonshot). Google needs a client of your own
  (docs/google.md).
- Accounts for coding agents (Claude Code, Codex, Gemini and others): signed in and out
  through the program that runs them, a permission that lasts one session ("This session only"),
  and a scoped key handed to a spawned agent that is revoked when it ends.
- Finding a server from an address; asking the person to confirm what was found; signing in
  again when a login expires; a seven-day reminder for a Google client still in testing; removal
  that also revokes at the service.
- The sign-in and consent sheets are drawn by the desktop; the Settings pages are fed by
  `org.quire.SettingsModule1`.
- An audit trail of what was released to whom.

### Sync (`syncd`)

- Calendar, contacts and tasks kept as local copies for Microsoft, Google, Nextcloud and
  other servers; a storage folder on Microsoft OneDrive, Google Drive's app folder and file servers such as Nextcloud;
  Photos (upload from a folder, the picker) behind `SYNCD_PHOTOS=on`.

### AI (`inferd`)

- A broker for language, speech and computer-use models: on this computer (engines it starts, or
  ones you already run), or a hosted one through an account, under the person's limits on what may
  leave the machine and what may be spent.
- An endpoint for outside coding agents' model calls.

### Packaging

- `scripts/install.sh` installs the three services, their systemd user units, the D-Bus
  activation files, the provider files and the caller table, under `PREFIX` and `DESTDIR`.
- README, licence texts (MIT or Apache-2.0), a Linux CI workflow.

### Known gaps

- No shipped Microsoft client id yet (the owner registers the app); until one is set, Microsoft
  accounts say so. Google always needs your own.
- The tests that start a private session bus run only in the author's jail so far.
- `Broker::infer` in porter-infer and text-to-speech in inferd are not built.

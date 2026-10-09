# Architecture

porter is the desktop's account and capability service: apps ask for a capability ("an account
that can store files with a change feed"), never for a brand; the user signs in once; consent is
per app, account, capability and data class; refresh tokens never leave the daemon; local AI
runtimes are accounts too and win by default. The design is quire's
`design/31-ACCOUNTS.md` (the source of truth); this file is the map of the code that freezes its
interfaces. `CONVENTIONS.md` holds the rules; `FINDINGS.md` the open items.

Reading order: section 1 (find the crate), section 3 (find the home), section 4 (find the
trait), section 6 (copy the recipe).

## 1. Crates and allowed edges

| Crate | Purpose | I/O |
| --- | --- | --- |
| `prov` | provenance for the companion: the shared ids (`SessionId`, `RunId`, `TaskId`, `EntityId`, `ActionName`, `MessageId`, `ThreadId`), `Effect` (`Read < UndoableWrite < Outbound < Destructive < Execute`), the merged `Actor` (`AgentRole::Worker` beside `Cua`; `Actor::Acp { program }` for an external ACP agent), the one `Message` model with `AgentRef` and `Address`, the desktop-scope join (`Confidentiality::join`, `flow_to`, `desktop_admits`), the label lattice (`Label`, `Labelled`, `Quarantined`) and the confirmation `Witness`; re-exports `SpaceId`, `SpaceScope`, `AppName`, `AgentProgram`, `DataClass`, `UnixSeconds` | none |
| `porter-core` | the vocabulary: ids and Spaces (`SpaceId` with its typed view `SpaceKind`: `desktop`, a desktop-wide Space `DesktopSpace` (a bare slug), or an app's own Space `app:<app name>:<n>` (`LocalSpace`); `SpaceId::for_app` from an app's number and its link; `SpaceScope` with `open_to`, the ownership rule; the registry's `DesktopSpaceRecord`, `SpaceName`, `SpaceLook` (opaque, at most 1 KiB) and `SpaceChange`), `Account` (with its `endpoints`), `Family`, endpoints (`ServiceEndpoint`, `EndpointUrl`, `Tls`, `LoginName`, `EndpointProtocol`, `RelayPlan` with `RelayAuth` (`Password`, `AccessToken`, `Anonymous`: no credential, for a linked origin)), the capability vocabulary (computer use included), `Need` and `matches`, provenance and `effective`, `Restriction`, `Locality`/`Tier`/`Billing`, `DataClass`, consent (`Grant`, `decide`, `decide_key` (an app's own Space is its alone), `availability`, the sheet's ask and answer), `Credential` (`Password`, `OAuth`, `ApiKey`, `Bearer`, `KeyPair`)/`SecretKey`, `IssuedToken`, the sheet (`SheetView`, `SheetInput`, `Progress`, the stage machine, and the hand-typed mail server form: `manual_form`, `refit`, `form_problem`, `parse_manual`), `AuditEntry`, `Persisted`, `ByteStream` and the in-memory `duplex`, the wire protocol (`AccountsRequest` with `OpenAuthenticated` and `OpenLinked`, `AccountsReply`, frames), the time seam (`clock`: `Clock`, `SystemClock`, `FixedClock`), the plain rows of one computer lending its models (`lending`: `GuestAnswer`, `GuestRow`, `RowState`, `Approval`, `GuestAsk`, `ComputerCandidate`; porter-tailnet re-exports the first four, so a bus client names them without its files and toml) | none (`SystemClock` reads the wall clock only when a daemon calls it) |
| `porter-fs` | porter's file writer, std only: `atomic` (`AtomicWrite`: a write staged, synced, renamed over the target, the directory synced; `AtomicFile`: one owner-only file kept with its `.bak`). The only blocking file writer; `porter-core` reaches none of it | file system (blocking, std only) |
| `porter-provider` | provider files (`ProviderSpec` with its `matching`, `CapabilityRow.linked_origins` (`LinkedOrigin`: the hosts a service's pre-authenticated links may point at), `parse_provider`), `ProviderSet`, `Issuer` and its endpoints, the client registry's file format (`ClientsFile`), the `Provider`, `ProviderSession` and `SignIn` traits with `SignInStep`, `Signed` and `RevokeOutcome` | none |
| `porter-secrets` | the `Secrets` trait, the Secret Service attribute scheme, `MemorySecrets` (feature `testing`), `Oo7Secrets` (feature `oo7`), `KeyringSecrets` (feature `keyring`), `FileSecrets` (feature `test-keys`, TEST ONLY: one 0600 JSON file, never default, never in dist) | none today; oo7 behind its feature |
| `porter-sync` | the sync contract: `Replica` (with `quota`), `Quota`, `Cursor`/`Anchor`, `BaseVersion`, `Change`/`Tombstone`, `Conflict`, `DatasetKind`; the journal's vocabulary and pure rules (`ItemState`, `JournalItem`, `StoredConflict`, `reconcile`, `local_changes`); `MemoryReplica` (feature `testing`) | none |
| `porter-http` | the HTTP seam: `HttpRequest`, `HttpResponse`, the `Http` trait; `HyperHttp` (feature `hyper`); `stream` (feature `stream`, pure): HTTP/1.1 over a `ByteStream` a host dials, `StreamHttp` over a `Dial` with its `StreamLimits` | none by default |
| `porter-proxy` | the authenticated relay: per `EndpointProtocol` a pure machine (`ImapRelay`, `SmtpRelay`, `HttpRelay`, `SieveRelay`, `Pop3Relay`) and `relay` over `ByteStream`s and a `Connect` the host passes in; `TokioStream` (feature `io`) | none by default |
| `porter-oauth` | PKCE, the loopback redirect rules, `ClientRegistry`, renewal, the token, refresh and revoke exchanges over `Http`; `LoopbackServer` (feature `io`) | none by default |
| `porter-discover` | one function per `Discovery` kind: autoconfig, SRV and MX leads, well-known, JMAP session, Nextcloud OCS, port probes; the `Dns` seam | none |
| `porter-tailscale` | a client of Tailscale's LocalAPI (v1.80.0 field names): typed `Status` (the signed-in user, the tailnet, this node and the peers with owner, online, last seen, SSH keys), `WhoIs`, `Notice` (one line of the watch stream), `TailscaleError` (not installed, not running, refused, signed out, no such peer, malformed, timed out, each a plain sentence); `LocalApi` (feature `io`) speaks HTTP/1.1 with hyper over tailscaled's unix socket, the path overridable | none by default; with `io`: the socket, tokio, hyper |
| `porter-tailnet` | lending a computer's models to the person's other computers over their Tailscale network, and calling those of another: `judge` (who may ask, from Tailscale's own answer about the caller: the person's own, not this computer, not a tagged server, not shared in, unless the person allowed it), `Guests` (the answers the person gave, by node id, in a file only they read; the questions waiting), `Hello`, `Refusal` (plain sentences); with feature `io` `Observer` (follows this computer's Tailscale), `lend` (listeners on the network's own addresses, following them, judging every connection, limits per computer and in all), `Dialer` (asks who is at the address before every connection), `Relay` (a local socket that leads to one computer, checking on every connection) and `greet` (reads a hello) | none by default; with `io`: sockets, tokio, hyper |
| `porter-dav` | WebDAV requests (PROPFIND, sync-collection REPORT) and the parsed multistatus, sync and quota replies | none |
| `storage-gdrive` | `GdriveReplica`, porter-sync's `Replica` over a folder of Google Drive's app data folder (`spaces=appDataFolder`, scope `drive.appdata`): changes by `changes.list` with a page token as the anchor after a first `files.list` listing (a refused token is `AnchorExpired`), writes checked against the file's `version` first (Drive documents no conditional update), small files by multipart upload (up to 4 000 000 bytes) and larger ones by a resumable session, `md5Checksum` as the content hash, quota from `about`; its HTTP is any `Http` (in syncd `StreamHttp` over `OpenAuthenticated`, which adds the bearer: the upload host and a session URL are on the API's own origin, so no linked origin) | none |
| `storage-graph` | `GraphReplica`, porter-sync's `Replica` over a folder of OneDrive's app folder (`/me/drive/special/approot`): changes by delta query with the delta link as the anchor (a resync is `AnchorExpired`), writes guarded by `If-Match` and `conflictBehavior=fail`, simple upload up to 4 000 000 bytes and an upload session past it, quota from the drive; its HTTP is any `Http`; `Routed` sends the drive's origin to one and every other origin (an `uploadUrl`, a download's redirect) to a per-origin one (in syncd porter-http's `stream::StreamHttp` over `OpenAuthenticated`, which adds the bearer, and over `OpenLinked`, which adds nothing) | none |
| `storage-webdav` | `WebDavReplica`, porter-sync's `Replica` over one WebDAV folder: changes by sync-collection (RFC 6578, `sync-level` infinite) or an etag tree walk where the server has none, writes guarded by `If-Match`/`If-None-Match`, quota from RFC 4331; its HTTP is any `Http`, in syncd porter-http's `stream::StreamHttp` (an HTTP/1.1 client over a `Dial` of authenticated byte streams; syncd: accountd's `OpenAuthenticated`), re-exported here for one batch | none |
| `porter-families` | the protocol families as code, one feature each (`nextcloud`, `generic`, `microsoft`, `google`, `api_key`, `openrouter`, `agent_login`, `tailnet`: Tailscale on this computer, asked over its socket, no credential held), and `FamilyProvider` over them | none by default |
| `porter-infer` | the AI broker's pure half: requests and replies (chat with tools and controls, embeddings with a query/document role, tasks, computer-use steps, speech), `OpenOptions` (the reserved `traceparent` and `usage`: `interactive` or `background`, absent meaning `interactive`), the streaming session (`ClientFrame`, `InferEvent`, `InferSession`), the model picker's data (`Slot`, `TierMap`, `PickerRow`), `plan_pipeline`, `Readiness`, `Policy` and floors, `admit` (the hard rules), `route` and `pick` (Named or Automatic, warm first) with the `Why` of every answer, spend caps, `AuditEntry`, the `Model` trait, `Broker` (stubbed) | none |
| `porter-service` | accountd's core over its seams: `AccountService`, `Registry`, the `Sheets` (with `SheetLink`), `Clock`, `RegistryStore` and `AuditSink` traits | none (seams are passed in) |
| `porter-client` | the app-facing API: `Accounts` (with `open_authenticated`), `Found`, `AuthenticatedStream`, the `Transport` trait (`call` for accountd, `open_authenticated` for a relay; with feature `infer`, `open` for an inference session and `prepare`); `InProcess` (feature `in-process`, default on: the one carrier that hosts porter-service; with a `SessionHost` for inference under `infer`, `NoBroker` by default, and a `RelayHost`, `NoRelays` by default), `SocketTransport` (feature `socket`: the agent is a `SocketAgent`, its address, lock and start are latchkey's), `DbusTransport` (feature `dbus`; `spaces()` gives `Spaces`: `list`, `create`, and `watch`, a stream of `SpaceChange`s; `Accounts<DbusTransport>::watch_removals()` gives `Removals`, the accounts accountd removes); the computers and guests of inferd's `Inference1` are `Transport` calls with an `Unsupported` default (`guests`, `answer_guest(node, GuestAnswer)`, `forget_guest`, `candidates`, and under `infer` `add_tailnet_computer`, `add_computer(NewComputer)`, `remove_computer`), typed with `porter_core::lending`, a refusal being `TransportError::Computer { reason: ComputerReason, words }`, and `Accounts<DbusTransport>::watch_guests()` gives `GuestChanges`, a stream of `GuestChange` (`Asks`, `Changed`); feature `dbus` also has `porter_client::peer`, the daemons' typed way into accountd's `Peer` surface: `PeerAccounts` (`verdicts`, `resolve_key`, `report_local`, `news`), `PeerError` (the one reading of a bus failure for a daemon, built on `porter_dbus::classify`), `AccountVerdict`, `AccountNews`; both of those share one framed session over a Unix stream (feature `framed`); feature `lending` has `TailnetLending` (`enable`, `disable`, `state() -> LendingState`; errors `LendingError`), the Settings switch "Let my other computers use this computer's models" installing `share/porter/inferd-tailnet.conf` as inferd's drop-in with the person's consent (the `UnitManager` trait is injected; `SessionUnits` is the real one under `dbus`; it never sets `ai.tailnet.serve`). Feature `infer` (default on) is inference: without it (`default-features = false`) the crate reaches no porter-infer, so no stoker, and no zbus (quire design/36) | through its transport |
| `porter-dbus` | `org.quire.Accounts1` (with `Peer`), `org.quire.AccountsSheet1`, `org.quire.Sync1` (with `org.quire.Photos1.Picker` on the same name and object), `org.quire.Inference1` and `org.quire.Inference1.Agents` (inferd's agent endpoints), `org.quire.Spaces1` (the desktop-wide Spaces, on accountd's name) and `org.quire.Tailnet1` (the person's computers on their Tailscale network, on accountd's name; `machine_to_dbus` and `machine_from_dbus` are its row codec) as zbus proxies and skeletons; `introspection`; the argument codec; the sheet answer (`sheet`: request paths, response codes, results) and its caller's half (`pending`: subscribe before the call, `Closer`); `callers` (`CallerRole`, `CallerTable`, `ProcCallers`); `BusTarget` (the session bus or an address: how a daemon's `Config` names its bus); `names`, `callers` and `sheet` are public modules too (the root re-exports stay for one batch) | zbus |
| `porter-fake` | test-only: fake providers from real provider files, three accounts (with endpoints), `ScriptedSheets`, `FixedClock`, `MemoryStore`, `RecordingAudit`, the `FakeServer` seam, `FakeModel` (streams), `FakeInferSession` (scripted events per request kind, audio-gated transcripts), `fake_service` | none |
| `porter-fake-servers` | test-only (never a dependency of anything else, checked): fake servers on loopback or scratch Unix sockets, built from the shipped `providers/*.toml` with endpoints rewritten (`FakeServer::rewrite`), each recording what it received on a handle: a fake Google (an issuer in Google's style: one refresh token to a code that asked for `access_type=offline`, never rotated, the granted scopes in every answer, the application secret required; and the userinfo, Calendar, People, Tasks and Drive app-folder probes the Google family reads, `403` for a token whose scopes lack the service's), a Microsoft Graph drive (OneDrive's app folder: delta, ETags, simple and session uploads; it accepts one fixed bearer, or every access token a fake issuer minted and has not revoked), an OAuth issuer (authorize with PKCE S256 and state, token, refresh rotation, revoke, `invalid_grant`, device code) with a scripted browser, IMAP and SMTP (STARTTLS and implicit TLS with a scratch CA made by `fixtures/regen.sh` with the openssl CLI), a Nextcloud (Login Flow v2, OCS app-password delete, WebDAV with sync tokens and quota, CalDAV and CardDAV with principal, home sets, collection names and colours, Notes), a plain DAV server, autoconfig and well-known routes, `FakeDns` behind `porter-discover`'s `Dns`, and Ollama and OpenAI-compatible model lists | tokio, rustls (ring), loopback only |
| `porter-rig` | test tooling, never installed (`publish = false`, not in `dist/`, check-boundary: nothing names it): the rig a jailed scenario drives porter with. `porter-rig-servers` starts fakes on loopback ephemeral ports, plants known secrets (`planted`), writes `rig.json` and serves the scenario's levers over a loopback HTTP control endpoint; `porter-rig-client` acts as an app over the session bus (`request-grant`, `open`, `open-authenticated`, `sync-resolve`, `watch-conflicts`, ...) and writes the identity fixture the daemons' `test-proc-root` builds read; `porter-rig-secrets` is a fake `org.freedesktop.secrets` (plain algorithm, in memory) so the real accountd's `Oo7Secrets` runs with no keyring | tokio, zbus, clap, loopback only |
| `accountd`, `syncd`, `inferd` | the daemons. Each library has a `daemon` module: a typed `Config` (its `from_env` is the one place the library reads the environment; check-boundary greps for any other), `Daemon::build(Config)` and `Daemon::run(shutdown)`. Each binary only parses its arguments and calls those (syncd's holds the network stub, inferd's listens for signals, accountd's `add` subcommand prints what `accountd::add_account` returns). `syncd` is a library (`journal`, `engine`, `dataset`, `scheduler`, `driver`, `service`, `removal`, `paths`) and a binary that resolves its paths, loads the caller tables, runs the PIM supervisor and the storage supervisor (the Graph app folder mirror, and Photos behind `SYNCD_PHOTOS=on`), serves `Sync1` over its hub and listens for `AccountRemoved`; `accountd` is a library (`core`, `manager`, `grants`, `account`, `request`, `peer`, `settings` and `settings_keys`, `relay` (the `OpenAuthenticated` and `OpenLinked` socketpair and relay task), `sheets` (`BusSheets` over `org.quire.AccountsSheet1`), `hub` (who is told what), `paths`, `providers`, `callers`, `errors`, `spaces` and `spaces_object` (`org.quire.Spaces1` at `/org/quire/Spaces1`: the registry of desktop-wide Spaces in `spaces.json` beside `registry.json`, saved by the same `store::file::AtomicFile` with its `.bak`; legacy slugs adopted on the first start; `Create` limited per app; `Remove` ends the grants scoped to that Space): the bus objects over `AccountService`, the Request objects of the sheet methods, the caller table seam) and a binary that resolves its paths, loads its registry (refusing one it cannot read), caller tables and provider files, builds the service over `Oo7Secrets`, `FileStore`, `FileAudit`, `ProcCallers` and `BusSheets`, and serves (the Spaces in `SpacesStore::File` of the registry's directory); `inferd` serves `org.quire.Inference1` (section 10). `inferd` is also a library (`adapters`, `bridge`, `session`, `serve`, `service`, `peers`, `router`, `engines`, `supervise`, `hosts`, `local`, `catalog`, `config`, `runner`, `structured`, `tee`, `cua_step`, `cua_run`, `audit`, `clock`, `speech`, `replay`, `cloud`, `attached`, `agent`) so its modules are tested without a bus | everything |

Allowed direct edges (checked by `scripts/check-boundary.sh`; dev-dependencies are outside it):

| Crate | May depend on |
| --- | --- |
| `porter-core` | nothing of ours |
| `prov`, `porter-provider`, `porter-secrets`, `porter-sync`, `porter-dbus`, `porter-http`, `porter-proxy`, `porter-dav` (also `porter-http`) | `porter-core` |
| `porter-oauth`, `porter-discover` | `porter-core`, `porter-http`, `porter-provider` |
| `porter-tailscale` | `porter-core` (its `NodeId`, `Machine`, `MachineOwner`); with feature `io`, hyper, hyper-util, http-body-util and tokio for the socket |
| `porter-tailnet` | `porter-core`, `porter-tailscale` (with feature `io`: hyper, hyper-util, http-body-util and tokio for the sockets) |
| `porter-families` | `porter-core`, `porter-provider`, `porter-http`, `porter-dav`, `porter-discover`, `porter-oauth` (its `io` with `microsoft` and `google` only), `porter-proxy` (its `tls` feature, which implies `io`, with `generic` only: the relay's own login tries a typed mail password before the account is added), `porter-tailscale` (its `io` with `tailnet` only: asks Tailscale who is signed in); the `hyper` feature of `porter-http` with `api_key`, `microsoft` and `google` |
| `storage-graph` | `porter-core`, `porter-http` (feature `stream`: `StreamHttp`, `Dial`), `porter-sync`, `storage-webdav` (its `Clock`), serde, serde_json, base64 (used by syncd only; no consumer repo) |
| `storage-gdrive` | `porter-core`, `porter-http` (feature `stream`: `StreamHttp`, `Dial`), `porter-sync`, `storage-webdav` (its `Clock`), serde, serde_json (used by syncd only; no consumer repo) |
| `storage-webdav` | `porter-core`, `porter-dav`, `porter-http`, `porter-sync` (used by syncd only; no consumer repo) |
| `porter-infer` | `porter-core`, `cua-action` (stoker's computer-use vocabulary, by git rev) |
| `porter-bridge` | `porter-core`, `porter-infer`, and stoker's `model-provider`, `model-catalog`, `model-openai-compat`, `vision-prep` (pure: the one mapping between porter's wire types and stoker's turns, shared by inferd and porter-client's `engines`) |
| `porter-service` | `porter-core`, `porter-provider`, `porter-secrets` |
| `porter-client` | `porter-core`; `porter-provider`, `porter-secrets`, `porter-service` with feature `in-process` (default; `InProcess` is the only carrier that reaches them, and `dbus` alone reaches none, checked by check-boundary.sh); `porter-infer` with feature `infer` (default); `porter-dbus` and zbus (the `peer` module's news subscription) with feature `dbus`; latchkey (git, not ours) with feature `socket` (a Unix socket, or a named pipe on Windows); `porter-fs` with feature `lending` (off by default; `TailnetLending`, which installs and takes away inferd's tailnet drop-in for the Settings switch, with a `UnitManager` the caller injects and, with `dbus`, `SessionUnits` over the session bus; check-boundary.sh checks that `dbus` alone reaches no porter-fs); with feature `engines` (off by default; inference with no inferd: `engines::EngineHost`, a `SessionHost` over a routing table, a `Policy` and the app's `KeySource`) `porter-bridge` and stoker's `model-http` (`hyper`, `tls`), `model-openai-compat`, `model-provider`, `model-wire` |
| `porter-fake` | `porter-core`, `porter-infer`, `porter-provider`, `porter-secrets`, `porter-service` |
| `porter-fake-servers` | `porter-core`, `porter-discover`, `porter-fake`, `porter-provider` (and nothing may depend on it) |
| `porter-rig` | `porter-client` (feature `dbus`), `porter-core`, `porter-dbus`, `porter-fake`, `porter-fake-servers`, `porter-infer`, clap, serde, serde_json, tokio (with `signal`), zbus (and nothing may depend on it) |
| `accountd` | `porter-core`, `porter-dbus`, `porter-families`, `porter-provider`, `porter-proxy` (feature `tls`, for `OpenAuthenticated`), `porter-secrets`, `porter-service`, `porter-tailscale` (feature `io`: follows Tailscale for the account's state and `org.quire.Tailnet1`), and quire's `ds-settings` (feature `live`, by git rev like stoker) for `org.quire.SettingsModule1`: the one porter -> quire edge, accountd and inferd (its model picker), never a library crate (check-boundary forbids it everywhere else) |
| `syncd` | `porter-client` (feature `dbus`: `Accounts<T>` over a `Transport`, for `open_authenticated` and the PIM supervisor's `find`; the binary's `DbusTransport`; `Accounts::watch_removals` for `AccountRemoved`), `porter-core`, `porter-dav` (the PIM mirror's discovery), `porter-dbus`, `porter-http`, `porter-sync`, `storage-gdrive`, `storage-graph`, `storage-webdav` (and rusqlite, SQLCipher built from source with a vendored OpenSSL, used unkeyed) |
| `inferd` | `porter-client` (feature `dbus`: `peer::PeerAccounts`, how it asks accountd for grant verdicts and API keys, reports a local runtime and hears the account news), `porter-core`, `porter-dbus`, `porter-tailnet` and `porter-tailscale` (feature `io` of both: lending models to the person's other computers and calling theirs; `tailnet` is the wiring), `porter-discover` (the probe of local runtimes' ports), `porter-http` (feature `hyper`: the plain-HTTP client of that probe), `porter-infer`, `porter-provider` (`Port`), `cua-action`, quire's `ds-settings` (feature `live`, as accountd's, for the model picker), and stoker's `model-provider`, `model-catalog`, `engine-supervisor`, `model-http` (features `hyper` and `tls`), `model-openai-compat`, `vision-prep` (feature `pixels`), `cua-parse`, `cua-session`, `cua-vendors`, `model-extract`, `model-replay`, `model-wire` (the `Driver` over inferd's body-shaping wrapper of `HttpClient`), `speech-provider`, `speech-host-client`, by git rev |

Cross-repo dependencies are git deps at pinned revs, so a git checkout of porter builds with no
sibling checkout: stoker (`https://github.com/PoHsuanLai/stoker`, rev in the root `Cargo.toml`) and
quire's `ds-settings` (`https://github.com/PoHsuanLai/quire.git`, rev as quire's pinned block). To
work on them locally, put a git-ignored `.cargo/config.toml` in the porter root (never commit it):

```toml
[patch."https://github.com/PoHsuanLai/stoker"]
cua-action = { path = "../stoker/crates/cua-action" }
# ... one line per stoker crate porter names (the root Cargo.toml lists them)
[patch."https://github.com/PoHsuanLai/quire.git"]
ds-settings = { path = "../quire/crates/ds-settings" }
# plus every ds-* crate that resolves from the same rev, so one copy of each is built
```

A consumer that also takes quire or stoker by path needs the same `[patch]` in its own workspace
root, or its graph holds two copies of `ds-*` (the git one through porter, its path one).

External boundaries: every crate but `porter-dbus` and the daemons never reaches `zbus`,
`zvariant`, `tokio`, `reqwest`, `hyper`, `ureq`, `oo7`, `keyring`, `secret-service`,
`interprocess` or `latchkey` (default features); `porter-core` also never reaches `toml`.
`porter-http` reaches `hyper` and `tokio` only through its feature `hyper` (its feature `stream`
reaches none: check-boundary checks it), `porter-proxy` and
`porter-oauth` reach `tokio` only through their feature `io`. `porter-families` reaches none with
default features; through its family features it reaches exactly this (measured with
`cargo tree --no-default-features --features <f>`, and enforced per feature by check-boundary):
`generic` reaches `tokio` (porter-proxy's `io`, for the relay's own login); `microsoft` and
`google` reach `tokio` and `hyper` (porter-oauth's `io` and porter-http's `hyper`); `api_key`
reaches `hyper` and `tokio` (porter-http's `hyper`); `tailnet` reaches `hyper` and `tokio`
(porter-tailscale's `io`); `nextcloud`, `agent_login` and `openrouter` reach none (`openrouter`
takes porter-oauth without its `io`). A daemon or an app hosting porter turns those features on.
`porter-dbus` reaches `tokio` only through zbus's `tokio` feature (the pinned block's zbus line).
`tokio` is a direct dependency of the daemons only, and a dev-dependency of async tests.

## 2. Modules

| Crate | Modules |
| --- | --- |
| `prov` | `ids`, `effect`, `actor`, `agent`, `label`, `scope`, `message`, `consent`, `trace` (span attribute names, `slug`s) |
| `porter-core` | `id`, `app_id`, `units`, `space`, `error`, `family`, `stream`, `xdg` < `capability` (`terms`, `mail`, `pim`, `storage`, `photos`, `ai`, `sync_kinds`, `kind`) < `need` (`data`, `ai`) < `offer`, `effective`, `matching`, `restriction`, `ai_props`, `data_class`, `auth_kind`, `endpoint`, `account` < `consent` (`grant`, `decide`, `prompt`) < `secret`, `token`, `candidate` < `sheet` (`fields`, `progress`, `view`, `input`, `stage`), `audit`, `store` < `wire` (`request`, `reply`, `frame`) |
| `porter-provider` | `error` < `spec` (`auth`, `discovery`, `matching`, `linked`), `clients` < `parse`, `set` < `sign_in` < `provider` |
| `porter-secrets` | `error`, `attributes` < `secrets` < `memory`, `oo7`, `keyring`, `file` (`test-keys`) |
| `porter-sync` | `anchor`, `item`, `transfer`, `quota` < `change`, `refusal`, `dataset` < `journal` < `journal_reconcile`, `replica` < `memory` |
| `syncd` | `paths`, `clock` ; `journal` (`schema`, `rows`) ; `dataset` (`memory`, feature `testing`) < `engine` (`pull`, `push`, `resolve`) < `driver` ; `scheduler` (`backoff`) ; `service` (`hub`, `status`, `errors`) < `object` ; `removal` ; `webdav` (`RelayDial`, `webdav_replica`) ; `gdrive` (`gdrive_replica`) ; `datasets::pim` (`vdir`, `mirror` (`PimMirror`), `discover`, `plan`, `relay`, `grants`, `mirrors`, `supervisor`) ; `graph` (`graph_replica`, `LinkedDial`) ; `datasets::storage` (`folder` (`FolderDataset`), `grants`, `supervisor` (`StorageSupervisor`)) ; `datasets::photos` (`google`: `api`, `ledger`, `upload` (`UploadReplica`), `picker` (`PhotosPicker`), `wiring`) |
| `porter-http` | `error`, `headers` < `message` < `http` < `hyper_client` ; `stream` (feature `stream`: `client` (`StreamHttp`, `Dial`, `StreamLimits`) < `wire`, the HTTP/1.1 framing) |
| `porter-proxy` | `fault`, `step`, `connect` < `imap`, `smtp`, `pop3`, `http1` < `relay`; `tokio_stream` |
| `porter-oauth` | `pkce`, `loopback`, `registry`, `renewal`, `device` < `exchange`; `loopback_io` |
| `porter-discover` | `found`, `dns` < `autoconfig`, `well_known`, `ocs`, `probe` |
| `porter-dav` | `multistatus` < `request`, `sync` |
| `storage-webdav` | `path`, `clock`, `refuse`, `entry`, `requests`; `replica` < `feed`, `write` |
| `storage-graph` | `addr`, `json`, `refuse` < `replica` < `feed`, `write` < `upload` |
| `storage-gdrive` | `addr`, `json`, `refuse` < `replica` < `feed`, `write` < `upload` |
| `porter-families` | `skeleton` (the families not built yet) < `key` (what the cloud-AI key check shares) < one module per family (`nextcloud`, `generic`, `microsoft`, `google`, `api_key`, `openrouter`) < `dispatch` (`env_common`: the clock the caller passes in, randomness, and the clients files the two OAuth families share; no environment or system clock is read here) |
| `porter-infer` | `ids`, `control`, `open`, `request`, `cua`, `speech`, `reply`, `error`, `readiness` < `event`, `session`, `choice` < `policy`, `spend`, `audit` < `route` < `pick`, `model` < `broker` |
| `porter-service` | `clock`, `sheets`, `store`, `audit` < `registry` < `choose`, `token`, `audience` < `service` < `add` |
| `porter-client` | `error`, `env`, `found`, `authenticated`, `relays` < `transport` (`broker` (the `SessionHost` and `NoBroker`, with no service), `framed`, `in_process` (feature `in-process`), `socket`, `dbus`; each with its session) < `accounts` ; `peer` (`verdict`, `news` under `PeerAccounts`) and `removals` (feature `dbus`) over `porter-dbus` |
| `porter-dbus` | `names`, `args` < `codec`, `codec_grants` < `sheet` < `pending`, `callers` < `callers_file` (feature `callers-file`) ; `manager`, `account`, `grants`, `tokens`, `request`, `peer`, `launcher`, `sheet_backend`, `sync`, `spaces`, `inference`, `agents` < `introspect` |
| `inferd` | `session` (pure machine) < `serve` (`carried`, then the loop over four seams) ; `catalog` < `local` < `auto` (the `ai.auto.*` rows), `settings` (`Settings` and `Live`, `resolve` of the `ai.*` rows at their paths over the old `[policy]`/`[tiers]`, `ConfigFile` and `Reload`, and the live module `InferdSettings` at `INFERENCE_SETTINGS_PATH`; `engines` holds the `Live`), `swap` (the cost of loading a model now, from stoker's `budget`) < `router` < `supervise`, `hosts` < `engines` (`Engines`, `SessionRouter`) ; `bridge` (`request`, `reply`) < `tee`, `structured`, `cua_step`, `runner` ; `agent` (an external coding agent's model endpoint: `token`, `http` (the least HTTP/1.1), `fail` (errors in the protocol's own shape), `wire`, `anthropic` and `openai` (request read into `ChatRequest`, reply and event stream written from it), `route`, `meter` (usage read from a provider's reply, the ledger, the audit line), `account` (forwards to a provider with the real key) and `local` (runs on an engine through `runner`) under `handle`, `open` (the checks), `session` (the listeners), `refusal`, `service` (the `Agents` bus object)) over `cloud`, `engines`, `runner` and `audit` ; `cua_run` (the run and its `StepJob`) over `cua_step` ; `replay` (`cassette` < `replayer` < `engine`, `render`; `host`, `model`) beside `hosts`, feeding `main` ; `probe` (`names`, `entry`; the `[probe]` table, what answers on a port, the `LocalModel` of a probed model) < `probed` (the book of probed runtimes `Engines` routes through) ; `attached` (`config`: `[engines.attached."<id>"]` and its checks; `key`: the bearer's file; `target`: the endpoint of one connect; `check`: `GET /v1/models` and `NotReady`; `model`: the `LocalModel` of the catalogue's entry; `book`: the engines the person already runs and what the last look found, never in a supervisor) beside `probed`, read by `engines` (`Engines::with_attached`, looked at when a session opens, `want` re-looks), and `settings::MyNetwork` (`ai.attached.my_network`, read by `Settings::routing_policy`) ; `report` (`Peer.ReportLocal`) and `watch` (the look at start, on `Rescan` and on a timer with backoff: `probe`, then `probed`, then `report`) < `service` ; `audit`, `clock`, `peers`, `config` < `service` (the bus object) < `main` |

## 3. One home per concept

| Concept | Home |
| --- | --- |
| id grammar, D-Bus path segment | `porter-core::id` |
| caller identity | `porter-core::app_id` (`AppId`, `Isolation`) |
| a unit or count | `porter-core::units` |
| the capability vocabulary | `porter-core::capability` |
| what an app asks | `porter-core::need` |
| does an offer meet a need | `porter-core::matching::matches` (the only place) |
| effective capabilities (provenance, toggles) | `porter-core::effective` |
| why an account is limited | `porter-core::restriction` |
| consent decisions | `porter-core::consent::decide`, `availability` |
| credentials, where they are filed | `porter-core::secret`; attributes in `porter-secrets::attributes` |
| the wire protocol and its framing | `porter-core::wire` |
| a Space's id and scope | `porter-core::space` |
| who acted, labels, effects, the confirmation witness | `prov` |
| a message between agents (companion, worker, computer-use run, the person; across Spaces too), its thread, kind and typed parts | `prov::message` (there is no other return, result or report type) |
| the roster's agent reference and a message's address | `prov::agent` |
| how the `desktop` scope joins a Space's labels | `prov::scope` |
| the inference session (frames, events, replies) | `porter-infer::{request, event, reply}` |
| chat controls (tool choice, parallelism, output limit, reasoning, sampling, stop), `StopReason`, `ThoughtSeal` | `porter-infer::control` |
| the options of `Open` (the reserved `traceparent`) | `porter-infer::open`; the bus key is `porter-dbus::OPTION_TRACEPARENT` |
| span attribute names of our own (`quire.*`) | `prov::trace` (the OpenTelemetry `gen_ai.*` names are stoker's `genai-names`) |
| who retries, who repairs a structured reply | `inferd` (section 7) |
| the computer-use step contract | `porter-infer::cua` |
| the speech turn on the wire | `porter-infer::speech` |
| the model picker's data and the tier map | `porter-infer::choice` |
| which request kinds a session's need admits | `inferd::session::fits` |
| who is calling inferd (connection, pid, executable, caller) | `inferd::peers` (`CallerTable` from `inferd.toml`) |
| which model answers a session, and what it is pinned to | `inferd::router::choose` over `inferd::engines::Engines::route` (the only caller of `porter_infer::pick` and `porter_infer::route`) |
| the models this computer can run, from the stoker catalog | `inferd::catalog` (claims) and `inferd::local` (the book) |
| starting, probing and unloading an engine | `inferd::supervise` (the driver of stoker's `step`) over `inferd::hosts` |
| porter's request to stoker's turn, and back | `porter-bridge` (the only mapping; `inferd::bridge` adds the `LocalModel` targets) |
| inference in an app with no inferd | `porter-client::engines` (`EngineHost`: the app's table and policy, `porter_infer::admit` for the hard rules, the app's `KeySource`) |
| provider file format | `porter-provider::spec` + `parse` |
| which secret an auth kind presents | `porter-service::secret_purpose` |
| the account registry and candidates | `porter-service::registry` |
| the chooser/consent flow | `porter-service::choose` |
| the sync contract | `porter-sync::replica` |
| AI routing | `porter-infer::admit` holds the hard rules (the only place); `route` and `pick` call it |
| spend arithmetic | `porter-infer::spend` |
| D-Bus names and paths | `porter-dbus::names` |
| D-Bus argument shapes | `porter-dbus::args`, conversions in `porter-dbus::codec` |
| `Found` for apps | `porter-client::found` |
| the system clock | `porter-core::clock::SystemClock`, called only by the daemons (`accountd`, `syncd`, `inferd`); `porter_service::Clock` is the same trait |
| a family, and the protocol a relay speaks for it | `porter-core::family` (`Family`, `relay_protocol`, `serves`) |
| where an account's servers are | `porter-core::endpoint` (`ServiceEndpoint`); returned in `Candidate.endpoints` |
| which audiences a grant covers | `porter-service::audience` |
| the sheet's views, inputs and stage machine | `porter-core::sheet` |
| what accountd persists, and the migration table | `porter-core::store` |
| what accountd audits | `porter-core::audit` |
| the OAuth client registry's file | `porter-provider::clients`; the registry is `porter-oauth::registry` |
| a sign-in conversation | `porter-provider::sign_in` (`SignIn`, `SignInStep`, `Signed`) |
| HTTP requests and responses | `porter-http` |
| HTTP/1.1 framing over a byte stream, and the client that dials one | `porter-http::stream` (feature `stream`) |
| the authenticated relay's protocol rules | `porter-proxy` (`imap`, `smtp`, `pop3`, `http1`) |
| who is calling a daemon, and its role | `porter-dbus::callers`; the table read from its files, `porter-dbus::callers_file` (feature `callers-file`, on in accountd and syncd) |
| writing a file atomically (staged, synced, renamed, directory synced), and a file kept with its `.bak` | `porter-fs::atomic` (`AtomicWrite`, `AtomicFile`): the only writer; accountd, inferd, porter-secrets, porter-tailnet and syncd all call it |
| the XDG base directories from an injected environment | `porter-core::xdg` |
| a `Retry-After` of seconds | `porter-http`'s `HttpResponse::retry_after_seconds` |
| how an engine request is retried | `porter-bridge::ENGINE_RETRY` (inferd and porter-client's `engines`) |
| the sheet host's bus interface | `porter-dbus::sheet_backend` |

## 4. Traits (the seams) and closed enums

```rust
// porter-provider: one per protocol family, plus the fake.
pub trait Provider: Send + Sync {
    type Session: ProviderSession;
    fn spec(&self) -> &ProviderSpec;
    fn auth_kind(&self) -> AuthKind { self.spec().auth.kind }
    fn discover(&self, account: &AccountId, presented: &Presented)
        -> impl Future<Output = Result<Vec<Claim>, ProviderError>> + Send;
    fn open(&self, account: &AccountId, presented: Presented)
        -> impl Future<Output = Result<Self::Session, ProviderError>> + Send;
    fn sign_in(&self, start: SignInStart) -> Result<Self::SignIn, ProviderError>;
    fn revoke(&self, presented: &Presented)
        -> impl Future<Output = Result<RevokeOutcome, ProviderError>> + Send;
}
// porter-provider: one conversation per family; the host feeds `SignInInput`, draws each step.
pub trait SignIn: Send {
    fn next(&mut self, input: SignInInput) -> impl Future<Output = SignInStep> + Send;
}
pub trait ProviderSession: Send + Sync {
    fn access_token(&self, audience: &Audience) -> impl Future<Output = Result<IssuedToken, ProviderError>> + Send;
    fn renewed(&self) -> Option<Credential>;
}

// porter-secrets: oo7, keyring (macOS, Windows), the in-memory fake.
pub trait Secrets: Send + Sync {
    fn put(&self, key: &SecretKey, value: &Credential) -> impl Future<Output = Result<(), SecretsError>> + Send;
    fn get(&self, key: &SecretKey) -> impl Future<Output = Result<Credential, SecretsError>> + Send;
    fn delete(&self, key: &SecretKey) -> impl Future<Output = Result<(), SecretsError>> + Send;
    fn delete_account(&self, account: &AccountId) -> impl Future<Output = Result<(), SecretsError>> + Send;
}

// porter-sync: one per storage family, plus MemoryReplica.
pub trait Replica: Send + Sync {
    fn changes(&self, from: Cursor) -> impl Future<Output = Result<ChangePage, ReplicaError>> + Send;
    fn fetch(&self, item: &RemoteId, range: ByteRange) -> impl Future<Output = Result<Blob, ReplicaError>> + Send;
    fn put(&self, item: PutItem, base: BaseVersion)
        -> impl Future<Output = Result<(RemoteId, RemoteVersion), PutRefused>> + Send;
    fn remove(&self, item: &RemoteId, base: BaseVersion) -> impl Future<Output = Result<RemoteVersion, PutRefused>> + Send;
    fn features(&self) -> StorageCap;
}

// porter-infer: one per wire adapter, plus FakeModel.
pub trait Model: Send + Sync {
    fn card(&self) -> &ModelCard;
    fn chat(&self, request: &ChatRequest, sink: &mut impl ChatSink)
        -> impl Future<Output = Result<ChatReply, ModelError>> + Send;
    fn embed(&self, request: &EmbedRequest) -> impl Future<Output = Result<EmbedReply, ModelError>> + Send;
}

// porter-service: the sheet host (sill over AccountsSheet1, an app's own window) or
// ScriptedSheets; the system or a fixed clock; a registry file or none; an audit file or none.
pub trait Sheets: Send + Sync {
    type Link: SheetLink;
    fn consent(&self, ask: ConsentAsk, window: &ParentWindow) -> impl Future<Output = ConsentAnswer> + Send;
    fn conversation(&self, open: SheetOpen) -> impl Future<Output = Result<Self::Link, SheetFault>> + Send;
}
pub trait SheetLink: Send {
    fn update(&mut self, view: SheetView) -> impl Future<Output = Result<(), SheetFault>> + Send;
    fn input(&mut self) -> impl Future<Output = Result<SheetInput, SheetFault>> + Send;
}
// Consent and the sheet machine: `ConsentAnswer::AddAccount` ("Add Account…") makes the service run the
// add sheet with the alert's ask attached (add-and-allow, one grant); `ProviderRow.kind: RowKind
// { Provider, Generic }` marks the generic-* "Other…" row; `SheetInput::OpenAgain` on the browser
// step makes the machine emit `SheetEffect::OpenBrowser(url)`, which the service serves by showing
// `BrowserWait` again.
pub trait Clock: Send + Sync { fn now(&self) -> UnixSeconds; }
pub trait RegistryStore: Send + Sync {
    fn load(&self) -> impl Future<Output = Result<Persisted, StoreError>> + Send;
    fn save(&self, state: &Persisted) -> impl Future<Output = Result<(), StoreError>> + Send;
}
pub trait AuditSink: Send + Sync { fn record(&self, entry: AuditEntry); }

// porter-core: ordered async bytes without a runtime; tokio streams, the in-memory `duplex`.
pub trait ByteStream: Send { /* read, write_all, shutdown */ }
// porter-proxy: TCP and TLS, passed in by the host.
pub trait Connect: Send + Sync { /* dial(origin, tls), upgrade(stream, host) */ }
// porter-http: hyper in the daemon, a fake server's client in tests.
pub trait Http: Send + Sync { /* send(HttpRequest) -> HttpResponse */ }

// porter-client: D-Bus, the latchkey socket, in process.
pub trait Transport: Send + Sync {
    // `Session`, `open_with`, `prepare` and `open` exist with feature `infer` (default on)
    type Session: InferSession;
    fn call(&self, request: AccountsRequest) -> impl Future<Output = Result<AccountsReply, TransportError>> + Send;
    // provided: a transport that cannot carry the relay's descriptor is `Unreachable`
    fn open_authenticated(&self, grant: &GrantId, endpoint: &EndpointUrl)
        -> impl Future<Output = Result<Relayed, TransportError>> + Send;
    fn open_with(&self, need: &Need, class: DataClass, tier: Tier, options: &OpenOptions)
        -> impl Future<Output = Result<Self::Session, TransportError>> + Send;
    // provided: `open` is `open_with` with no trace context
    fn open(&self, need: &Need, class: DataClass, tier: Tier)
        -> impl Future<Output = Result<Self::Session, TransportError>> + Send;
}

// porter-infer (re-exported by porter-client): one per transport, plus FakeInferSession.
pub trait InferSession: Send {
    fn send(&mut self, frame: ClientFrame) -> impl Future<Output = Result<(), SessionError>> + Send;
    fn next(&mut self) -> impl Future<Output = Result<InferEvent, SessionError>> + Send;
}
pub trait ChatSink: Send { fn event(&mut self, event: InferEvent) -> Flow; }
```

Closed sets stay enums: `Capability`/`CapabilityKind`/`Need` (versioned by `VocabVersion`),
`AuthKind`, `Family`, `EndpointProtocol`, `Issuer`, `Discovery`, `DataClass`, `Provenance`, `AbsentReason`,
`Locality`, `SecretPurpose`, `AccountsRequest`/`AccountsReply`/`Refusal`,
`InferRequest`/`ClientFrame`/`InferEvent`/`InferReply`/`InferRefusal`, `Slot`, `Readiness`, `DatasetKind`, `Found`.

## 5. What is frozen, what is built, what is stubbed

Frozen means: the types, trait signatures, wire and file formats and D-Bus signatures below are
the interface other work builds on; a change is a vocabulary bump (section 6) or a design/31 edit.

| Piece | State |
| --- | --- |
| capability vocabulary, needs, `matches`, `effective`, restrictions, AI properties | built, table-tested |
| consent: `decide`, `availability`, the sheet's ask/answer | built, table-tested |
| ids, `AppName`, `LanguageTag` parsing; credential redaction | built, tested |
| wire enums and socket frames | built, round-trip tested |
| provider file format, parser and checks, `ProviderSet` | built, tested; the shipped files are in `providers/`, one per service or brand (`local.toml` is the supervised engines; `tests/shipped_files.rs` in porter-provider reads every one) |
| `Secrets` trait, attributes, `MemorySecrets` | built, tested |
| `Oo7Secrets`, `KeyringSecrets` | built (no `todo!()`); `Oo7Secrets` is tested over oo7's file backend and by the daemons' scenarios, `KeyringSecrets` through keyring-core's mock (FINDINGS, "Stubs behind frozen interfaces", says what is still unrun against a live store) |
| sync contract and `MemoryReplica` | built, contract-tested |
| routing, floors, spend arithmetic | built, table-tested |
| `Broker::infer` (streaming into a `ChatSink`) | stub |
| `prov` ids, `Effect`, `Actor`, `Quarantined`, `ReaderKey`, `Labelled::map` | built, round-trip and redaction tested |
| `prov::{Message, AgentRef, Address}`, `Message::check`, `AgentRef::of`, `Confidentiality::{join, flow_to}`, `desktop_admits` | built, round-trip, pinned-JSON and table tested |
| `prov` lattice: `Label::join`, `trusted_user`, `untrusted`, `Labelled::zip`, `endorse`, `declassify` | built |
| porter-core: `SpaceId`, `SpaceScope`, `GrantKey.space`, `Grant<K>`, `decide<K>`, computer use, `DataClass::Voice`, `VocabVersion(3)`; `VocabVersion(5)`: the `agent` capability kind (`AgentCap`, `AgentNeed`, `Subject::Agent`, family `acp_agent`), `AuthKind::AgentLogin`, `AccountState::NeedsLogin`; `VocabVersion(8)`: `GrantScope::Session(LauncherSession)` ("This session only": `GrantScope` is no longer `Copy`), `ConsentAsk.session`, `CredentialEnd::SessionClosed`, `AuditEvent::SessionGrantEnded` | built, table-tested |
| porter-infer wire: tools, images, `ChatControl`, `StopReason`, `ThoughtPart`, `ReplyShape::Choice`, `EmbedRole`, `CuaBegin`/`CuaStep`, `Transcribe`/`Speak`, `ClientFrame`, `InferEvent`, replies | built, round-trip and pinned-JSON tested |
| `OpenOptions`, `Traceparent`, the `options` dictionary of `Inference1.Open`/`Prepare`/`Availability` | built (inferd reads `traceparent` and `usage` (`OPTION_USAGE`, the `consent::Usage` slug; absent is `interactive`, an unknown slug is invalid args) into `SessionSpec.usage`, and ignores other keys; it writes no spans yet) |
| `prov::trace` names, `ActorKind::slug`, `Effect::slug` | built, tested against the serde forms |
| `tier_choice`, `Slot::setting_key`, `InferRequest::kind` | built, table-tested |
| `picker_rows` | built, table-tested |
| `StageNote` on the wire (`porter-infer::event`) | built: `role`, `served`, `why` and `name: Option<ModelLabel>` (the catalogue entry's label, serde default and skipped when absent, so a payload without it decodes and a note without a name is the bytes it always was); every chat or task turn sends an `Answer` note after `Routed` and before its first token, a pipeline one per stage; pinned-JSON and old-payload tested |
| `inferd::session::step` | built (Routed/Waiting events, audio effects, cua progress, one queued request); `fits` built |
| `inferd::serve` (`serve_session` over `Router`, `EngineHost`, `TurnRunner`, `AuditSink`) | built, tested over scripted seams |
| `inferd::service` (`Inference1`: `Open` with the caller check, `Availability`, `Prepare`, `Usage`, `Rescan`, `EnginesChanged`, `Gpu`), `peers`, `config`, `main` | built; tested on a private bus with fake engines (`tests/hosted.rs`); `Usage` answers the caller's spend (empty on a daemon with no hosted models) |
| the real seams: `router` and `engines::SessionRouter`, `engines::Engines` over `supervise` and `hosts`, `runner::Turns` over `bridge` and `cua_step`, `audit` | built; the engine host is child processes (`ProcessHost`), not systemd transient units |
| `inferd::{probe, probed, report, watch}` (local runtimes as accounts: Ollama on `:11434`, llama.cpp on `:8080`, LM Studio on `:1234`, ports from `[probe]` in `inferd.toml`; each found one is a `ProbedBook` entry, its models `LocalModel`s served by plain HTTP to loopback (`LocalModel.loopback`) and listed on-device, ready while the runtime answers and `Unavailable` after; `Peer.ReportLocal` makes it an account and then `offline`) | built; tested over loopback fakes and on a private bus (`tests/probed.rs`, acceptance 6) |
| `inferd::speech` rules (`check_audio`, `audio_ms`), `SttBackend::SpeechHost`, `SpeechRunner::{for_model, transcribe}`, `Ears`, `pipeline::Hearing` | built: a `Transcribe` turn runs on the speech host (a supervised CPU engine, probed with `Hello`), tested against a fake host on a Unix socket (`tests/speech.rs`); a voice chat on a language session (`Transcribe`, audio, `EndOfAudio`, `Chat`) is planned with `plan_pipeline` and run by `run_pipeline`: `Ears` hears it, then the text model answers the transcript, one `Stage` note per stage (`tests/voice_chat.rs`); `SpeechRunner::speak` stub |
| `inferd::{catalog, local, bridge}` (the catalog read, its claims, the model book, both halves of the mapping to stoker's turns) | built, table-tested |
| `inferd::cua_run::{CuaRun, StepJob}` | built over `cua_step`, which drives stoker's `CuaSession` (`begin`, `request`, one turn through a `TranscriptSink`, `absorb_for` in place on the run's one session, one repair turn); `runner::Turns` runs a computer-use step through it (tool and text dialects) |
| `FakeInferSession`, `FakeModel` streaming | built, tested |
| `AccountService`: Query, Availability, Choose, ListGrants, Revoke, IssueToken, `remove_account` | built over the seams, tested end to end with the fakes |
| `AccountService`: AddAccount, Reauthenticate, the credential of `open_authenticated` | built over the `Sheets` and `SignIn` seams, tested end to end with the fakes and on a private bus |
| `AccountService`: the audience check of `IssueToken`, `Registry::relay_target` (a relay dials only an endpoint the account holds for the grant's kind), `Candidate.endpoints`, saving the registry through `RegistryStore` and auditing through `AuditSink` after every change | built, table-tested over the fakes |
| `porter-core`: `endpoint`, `sheet` (views, inputs, `Sheet::new`, `Sheet::view`), `audit`, `store` (the document and its migration table), `stream` (`ByteStream`, `duplex`) | built, round-trip and table tested; `sheet::step` (the pure stage machine) is filled |
| `porter-provider`: `matching`, the clients file, `SignInStep::progress`; `Provider::{sign_in, revoke}`, `Issuer::endpoints` | built; the families implement the seam |
| `porter-sync`: `Replica::quota`, `Quota` | built (`MemoryReplica` counts bytes and takes a limit) |
| `porter-proxy`: the IMAP capability and `PREAUTH` rules, the SMTP `EHLO` reply and mechanism choice, `TokioStream`, the `Debug` that hides bytes | built, table-tested, and the relays (`relay`, the IMAP, SMTP, ManageSieve, POP3 and HTTP/1.1 machines, `rewrite_head`) are built and tested over loopback fakes |
| `porter-oauth`: PKCE values, `ClientRegistry`, `renewal`, the device-code answer | built, tested: the exchanges, `parse_redirect` and `LoopbackServer` too |
| `porter-http`: the request, response and header values, header redaction | built, tested; `HyperHttp` is built (feature `hyper`); the stream client and framing are built, tested and moved from storage-webdav (feature `stream`) |
| `porter-discover`, `porter-dav`, `porter-families` (and each family's provider, session and sign-in) | built and tested over fakes; one family, `openrouter` (`porter-families/src/openrouter`), is still a skeleton whose bodies are `todo!()` (its key is signed in by the `api_key` family today) |
| `porter-dbus`: `Peer`, `AccountsSheet1`, `CallerRole`, `CallerTable` | frozen and introspection tested (`dbus/*.xml`); `ProcCallers::caller_of` is built (reads `/proc/<pid>/cgroup`) |
| `porter-client`: `Accounts::open_authenticated`, `AuthenticatedStream`, `InProcess` over a `RelayHost`, `DbusTransport::open_authenticated` | built, `SocketTransport::open_authenticated` too (behind feature `socket`) |
| `Accounts` (client API), `found`, `InProcess` accounts calls | built, tested end to end |
| `Accounts::connect`, `DbusTransport::open_with` and `call` (the sheet methods too: `Choose`, `AddAccount`, `Reauthenticate` through a Request object), `DbusSession` | built; tested over a private bus against a fake inferd, the real session server and the real `AccountService` behind a bus adapter |
| `SocketTransport` and `SocketSession` | built behind feature `socket` (Unix sockets; without it, or on Windows, nobody is reachable), tested against a hand-written agent; the agent itself is not built |
| `InProcess::open_with` | built over a `SessionHost` the app hands in (`with_broker`); with none, `Unreachable`; porter's own `Broker` is a stub |
| the Request objects of the sheet methods (`accountd`), the caller's half (`porter_dbus::Sheet`) | built, tested on a private bus (races, forged signals, close, leaving callers) |
| D-Bus proxies and skeletons, introspection files in `dbus/` | frozen, introspection tested; skeleton methods answer `NotSupported` |
| D-Bus argument codec (needs, candidates, grants, tokens, refusal error names) | built, round-tripped over the wire signature |
| `accountd` (the bus objects, as a library) | built and tested on a private bus: `Manager` (with the five unicast signals), `Grants`, `Tokens`, `Account` (the properties, readable only with a grant, and `Reauthenticate`), `Peer` (`Verdicts`, `ResolveKey`, the API key on a sealed memfd, and `ReportLocal`, a probed local runtime as an account; role `PorterDaemon` only; and `SetAgentState`, what an agent program says of its own sign-in for an `AgentLogin` account (no credential is held for it), role `AgentLauncher` only, a role that can call nothing else and that `dist/callers.toml` never grants; and the launcher's other six, `RegisterLauncher(programs as)`, `ReportAgentLogin(request, outcome, reason)`, `ReportAgentLogout(request, outcome, reason)`, `BeginSession(session)` and `EndSession(session)` (a launcher session lives with the connection that began it, and a grant scoped to it, `GrantScope::Session`, goes with it, with the process credentials under it) and `RequestAgentGrant(program, kind, class, session, parent_window, options)` (the consent sheet for an agent program's key, offering "This session only" when it names an open session of the caller), with the two signals `AgentLoginRequested` and `AgentLogoutRequested(request, account, program)` that accountd sends to the registrant of the program ALONE: `Account.Reauthenticate` on an agent account asks the launcher instead of running a sign-in, and Settings' `sign_out` asks it to sign the agent out. The login itself never reaches accountd or the bus (no token, no URL, no code): the launcher answers with `ready`, `failed` plus a word of a closed set (`LoginFault`), or `cancelled`, and nothing else is accepted (`launchers`; porter-client's `Launcher` is the launcher's side, behind its `dbus` feature), the Request objects, `org.quire.SettingsModule1` at `porter_dbus::ACCOUNTS_SETTINGS_PATH` (role `Settings` only; it broadcasts `Changed("accounts.<id>.state", <state slug>)` whenever an account's state changes, from `Core::publish` beside the State `PropertiesChanged`, as well as for a `Set`), `BusSheets`; an `Agent` is refused what acts for the person; `Tokens.OpenLinked` (a relay with no credential to an origin the provider file declares in `linked_origins`; same role check as `OpenAuthenticated`); `Tokens.OpenAuthenticated` is served (the grant and endpoint checked, then a socketpair whose far end runs `porter_proxy::relay`) |
| `accountd` (the binary) | built; `ACCOUNTD_PROC_ROOT` is honoured only by a `test-proc-root` build; `ACCOUNTD_KEYS=file:<absolute path>` (a 0600 plain-text key file instead of the Secret Service) only by a `test-keys` build, which refuses any other value and a loose-mode file at startup, while a shipped build ignores the variable and says so once (`keysel`); `accountd add <provider-id> [--allow <app-id>]... [--class <class>]...` adds an account from a terminal (`add`: a terminal `Sheets`, the key read with echo off) and holds the bus name `org.quire.Accounts1` while it runs, which is the lock against a running daemon; `Peer.ResolveKey` reads through `keys::KeyDesk` (`SecretsDesk` over the secret store, an audit sink and a clock) |
| `syncd` (the engine) | built and tested: the SQLite journal (versioned schema, migrations), the engine over a `Replica` and a `Dataset` (paged feed, anchor expiry by content hash, base versions, stored conflicts, tombstones until acknowledged, every step resumable: a table over every cut point), the pure scheduler (push or poll, idle and failure backoff, seeded jitter, batched wake-ups, metered pause), `Sync1` (`Datasets`, `Status`, `Pause`, `Resume`, `Resolve`, `ConfirmDiscard`; signals `Progress`, `Conflict`, `NeedsConfirmation`) served on the session bus (callers by `ProcCallers`, `STATUS_KEY_QUOTA`, unicast signals) and the `AccountRemoved` wipe. The WebDAV replica is wired as `syncd::webdav` (relays from `OpenAuthenticated`) and proven end to end on a private bus against the fake Nextcloud; the daemon's PIM supervisor (`syncd::datasets::pim`, W6e) mirrors every Calendar and Contacts grant syncd holds into `$XDG_DATA_HOME/porter/vdir/<account>/<collection>/` (`<uid>.ics`, `<uid>.vcf`, `displayname`, `color`; server to disk only, atomic writes) through relays to the CalDAV/CardDAV endpoint, and is proven end to end on a private bus against the fake Nextcloud; the Photos dataset (`syncd::datasets::photos`, W6f: originals named by SHA-256 under `<account>/originals/ab/<sha256>`, an HLC-stamped per-device manifest `manifest/<device>.json` merged field by field, `PhotoLibrary` with `import`/`export`/`manifest`/`edit`, off by default behind `PhotosSwitch`, wiped by `AccountRemoved` with the journals and mirrors) is proven by two machines against one fake WebDAV (acceptance 5); the Graph replica is wired as `syncd::graph` (the same relays, plus `LinkedDial` over `OpenLinked` for the upload session and download hosts) and proven end to end on a private bus against the fake Graph drive, whose links are on a second loopback origin, and the daemon runs it: `syncd::datasets::storage::StorageSupervisor` mirrors the app folder of every Microsoft account holding a Storage grant of class Files two ways into `$XDG_DATA_HOME/porter/storage/<account>/` (dataset `<account>/storage_app_folder`, owned by `org.quire.Files`, its conflicts settled with `Sync1.Resolve`) and, with `SYNCD_PHOTOS=on`, the Photos datasets for a grant of class Photos; a grant that goes stops the mirror and keeps its files, `AccountRemoved` wipes them; W6d adds the other storage replicas |
| `syncd` (the binary) | built; `SYNCD_PROC_ROOT` is honoured only by a `test-proc-root` build; `dist/syncd.service` and `dist/dbus/org.quire.Sync1.service` install it |
| `inferd` | built: a daemon on the session bus (the checked-in `dist/` files install it) |
| protocol families, wire adapters, the daemon's registry file and audit file | built (the registry is a JSON file with migrations, the audit a JSONL file); the one skeleton left is the `openrouter` family above |

## 6. Recipes

**Add a capability kind or a field** (a vocabulary bump): a design/31 §2 row first; the struct in
`porter-core::capability`, its variant in `Capability` and `CapabilityKind`, the need in
`porter-core::need`, its arm in `matching::fit` with a table row per field, round-trip rows in
`porter-core/tests/round_trip.rs`; bump `VocabVersion::CURRENT`; the D-Bus codec's field names. A bump costs a migration now that
the registry is persisted: add the step from the old version to `MIGRATIONS` in
`porter-core::store` with a fixture of the old document in `store/tests.rs`
(`every_version_since_the_first_stored_one_has_a_migration` fails until you do).

**Add a provider**: a file `providers/<id>.toml`; `tests/shipped_files.rs` in porter-provider
parses it. No code unless it needs a new family, issuer or discovery kind.

**Add a protocol family**: its `Family` variant; a crate or module implementing `Provider` and
`ProviderSession` and `SignIn` (a module in `porter-families` behind its own cargo feature; HTTP
goes through `porter_http::Http`); its variant in `FamilyProvider` (`dispatch.rs`); a conformance
test against a recorded fake (`FakeServer`).

**Add an auth kind**: its `AuthKind` variant; its row in `secret_purpose`; its sign-in steps in
the family's `SignIn`, and the `Progress` and `SheetView` states they need in `porter-core::sheet`.

**Add an accountd request**: the variant in `AccountsRequest` and its reply in `AccountsReply`
(a request that carries a descriptor, as `OpenAuthenticated` does, gets a `Transport` method of
its own and is `Unavailable` in `handle`); its arm in `AccountService::handle`; the D-Bus member in `porter-dbus` (proxy and skeleton), then
regenerate and review `dbus/org.quire.Accounts1.xml`; a client method in `Accounts`.

**Run a scratch accountd** (private bus, no Secret Service, no real home): build with
`--features accountd/test-keys` (add `accountd/test-proc-root` when the daemon must tell its
callers apart without service cgroups; both are test-only and never in dist). accountd looks for
provider files in `/usr/share/porter/providers`, then `$XDG_DATA_HOME/porter/providers`, then each
`--providers DIR` (later directories win; `$XDG_DATA_DIRS` is not read), so a scratch run points
`XDG_DATA_HOME` at its scratch dir or passes `--providers`. Callers come from
`/etc/porter/callers.toml` then `$XDG_CONFIG_HOME/porter/callers.toml`; the registry is
`$XDG_STATE_HOME/porter/registry.json`.

```sh
S=/path/to/scratch                                # an empty directory you own
dbus-daemon --session --fork --print-address --print-pid   # a private bus; kill it by its PID
export DBUS_SESSION_BUS_ADDRESS=<the address printed>
export HOME=$S XDG_STATE_HOME=$S/state XDG_CONFIG_HOME=$S/config XDG_DATA_HOME=$S/data
export XDG_RUNTIME_DIR=$S ACCOUNTD_KEYS=file:$S/keys.json   # absolute; created 0600
mkdir -p $XDG_CONFIG_HOME/porter $XDG_DATA_HOME/porter/providers
cp providers/openrouter.toml $XDG_DATA_HOME/porter/providers/   # or pass --providers DIR
printf '[[caller]]\napp = "org.quire.Inference"\nrole = "porter_daemon"\n' \
  > $XDG_CONFIG_HOME/porter/callers.toml
# the key on standard input (echo is off only on a tty), then `y` to "Add this account?"
printf '%s\ny\n' "$(cat openrouter.key)" | accountd add openrouter --allow org.quire.Docket
accountd &                                         # serves org.quire.Accounts1; stop it by PID
```

`add` checks the key with one call to the company (the provider file's `endpoint`): a scratch run
with no network points that line at a loopback fake (`porter-fake-servers` `FakeLlmApi`;
`accountd/tests/file_keys.rs` does exactly that). `add` holds the bus name while it runs, so stop
the daemon first and start it after. The key file is plain text: delete the scratch directory when
done.

**Add a dataset**: its `DatasetKind` variant and conflict rule; the dataset plug-in in syncd.

**Add a request kind** (a vocabulary bump when it changes a frozen type): its variant in
`InferRequest` and `RequestKind`, its arm in `kind()`, a row in `inferd::session::fits`, its
reply in `InferReply`, its events in `InferEvent` if it streams, the round-trip rows in
`porter-infer/tests/frames.rs`; a design/31 §5.5 line. Speech and computer use are the models:
they travel on the same `Open` fd and add no D-Bus member.

**Add a slot to the picker**: its variant in `Slot` with its slug (and stoker's catalogue `Slot`
with the same slug), the `ai.model.<slot>.<tier>` rows in design/22 and inferd's settings keys;
the `Need` it maps to in the comment on the enum.

**Add a data class**: its variant in `DataClass`, its floor in `Policy::proposed` (or a line in
`every_data_class_has_a_floor_decision` saying it goes anywhere), the `ai.floor.<class>` row.

**Add a wire adapter**: a `Model` implementation; its variant in `inferd`'s `AdapterModel`.

## 7. Test harness

> A test path below written `tests/<name>.rs` (or `tests/common/...`) means `tests/it/<name>.rs`:
> a crate's integration tests are modules of one executable, `tests/it/main.rs` (CONVENTIONS.md,
> item 5). The exceptions are the separate targets named there, each with its reason in `Cargo.toml`.

`porter-fake` is the harness: `fake_service(ScriptedSheets::answering([...]))` builds an
`AccountService` over the three fake providers (declared by `crates/porter-fake/providers/*.toml`),
`MemorySecrets` with their secrets filed, and `FixedClock(NOW)`. An app is
`Accounts::over(InProcess::new(service, app_id))`. `porter-client/tests/end_to_end.rs` is the
model (`in_process_session.rs` for a hosted broker). Tests never touch the real bus, the network, a keyring or the user's files. The bus tests
(`porter-client/tests/{dbus_open,dbus_served,dbus_accounts,dbus_sheets,accountd_requests,connect}.rs`) start a private
`dbus-daemon` from a scratch config (`tests/common/bus.rs`: cleared environment, scratch HOME and
runtime directory, killed on drop; `dbus-daemon` must be on `PATH`) and serve a fake `Inference1`
(`tests/common/inferd.rs`), the real session server over scripted seams
(`tests/common/served.rs`) or the real accountd front end (`accountd::serve`) over the real
`AccountService` (`tests/common/accountd.rs`, clients introduced through `TableCallers`;
`tests/common/racing.rs` is a hand-written `Manager` that answers before it replies and lets a
stranger answer first). `tests/socket.rs` runs `SocketTransport` against a hand-written agent on a
Unix socket in a scratch directory (`tests/common/agent.rs`, the real service and the real session
server behind it); a sheet that waits on the person is `Scripted::Hang` in `porter-fake`, whose
`AskLog` counts the asks that were dropped. The one place the environment names a bus
(`Accounts::connect`) runs in a child process the test starts with a private bus address.

`inferd`'s tests: the unit tests are in `src/*/tests.rs` (tables; the supervisor driver runs with a
paused clock over stoker's fakes or a recorder, and `hosts` over harmless child processes such as
`sleep`); `tests/serve.rs` is the session server over scripted seams; `tests/hosted.rs` is the whole
daemon: a private `dbus-daemon` (`tests/hosting/bus.rs`), the real `Inference1` object with the
client connection introduced as an app (or cuad) through a `TablePeers`, the real router, driver,
runner and audit sink, and fake OpenAI-compatible engines on Unix sockets (`tests/hosting/engine.rs`)
that keep the requests they were sent. The engine host there is a recorder that starts nothing, so
no process runs and no GPU is touched; the weights are empty directories in a scratch HOME.

## 8. The mailo mapping

mailo keeps working standalone; the mailo session migrates it onto porter (in-process first,
D-Bus on the desktop). Where each mailo piece lands:

| mailo (read-only) | porter |
| --- | --- |
| `mail-domain` `AccountId` (UUID) | `porter_core::AccountId` (its hyphenated lowercase text is a valid id) |
| `AccountPlan` (configured) | `Account` + the provider's `ProviderSpec`; the servers are `Account.endpoints` (`ServiceEndpoint`: family, url, `Tls`, login), returned in `Candidate.endpoints`; the protocol's own detail stays in mailo |
| `AccountCaps` (discovered IMAP detail) | stays in mailo; its summary is a `Claim` of `Capability::Mail` at `Provenance::Discovered` |
| `AuthPlan::OAuth { issuer, scopes }` | `AuthKind::OAuthPkce` + `Issuer`; scopes follow from the granted kinds; client ids stay deployment config per issuer and channel |
| `AuthPlan::Password { username, sasl }` | `AuthKind::Password` or `AppPassword`; the username is `ServiceEndpoint.login`; the password never reaches mailo: its engines take a stream from `Accounts::open_authenticated` (the relay logs in) |
| `OAuthIssuer { Google, Microsoft }` | `porter_provider::Issuer` |
| `SecretKey { account, purpose }` | `porter_core::SecretKey` |
| `SecretPurpose::{IncomingPassword, OutgoingPassword, OAuthRefresh}` | the same variants |
| `SecretPurpose::AddressBook` | `SecretPurpose::ServicePassword(CapabilityKind::Contacts)` |
| `SecretPurpose::{OpenPgp, Smime}` (keyed by fingerprint) | not porter's: mail signing keys stay in mailo (FINDINGS) |
| `Credential::{Password, OAuth}` with `chrono` expiry | `porter_core::Credential::{Password, OAuth}` with `UnixSeconds`; the redacting `Debug` carries over |
| `Credential::{OpenPgp, SmimeKey}` | stay in mailo, as above |
| `mail-runtime` `Secrets` (sync, `get`/`put`/`forget`) | `porter_secrets::Secrets` (async, `get`/`put`/`delete`/`delete_account`); `MapSecrets` is `MemorySecrets`, mailo's `KeyringSecrets` becomes `porter_secrets::KeyringSecrets` (macOS, Windows) or `Oo7Secrets` (Linux) |
| `oauth.rs`, `signin.rs` (registry), `renewal.rs`, `loopback.rs` | `porter-oauth` (PKCE, loopback rules, `ClientRegistry`, renewal, exchanges) behind `ProviderSession::access_token`/`renewed`; apps get `IssuedToken` (`Bearer`, `Xoauth2`) instead of credentials |
| `discover.rs` + `mail-proto` autoconfig | `porter-discover` (one function per `Discovery` kind, the `Dns` seam) behind `Provider::discover` |
| `mail-pim::dav` reply parsing | `porter-dav` |
| `ui/add_account/flow.rs` (the stage machine) | `porter_core::sheet` (`Sheet`, `Stage`, `step`); the view is quire's `ds-shell::accounts`, mapped from `SheetView` |
| the keyring entries `service=mailo` | not moved: accountd holds every account and the person signs in again (there is no `Adopt`) |
| `presets/` (provider table) | provider files in `providers/`, claiming addresses through `ProviderSpec.matching` |
| `latchkey` (agent lifecycle, socket/pipe) | still latchkey (standalone repo): `SocketTransport` takes the agent's address, single-instance lock and start from it (`SocketAgent`); frames are `porter_core::wire` |

## 9. Repo rules

- **Gate** (check every exit code):

  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  cargo test --workspace --all-features
  ./scripts/check-boundary.sh
  cargo deny check licenses
  ```

- **No `unsafe`** anywhere (`unsafe_code = "deny"`).
- **Dependencies** come from quire's pinned block (`docs/workspace-deps.toml` there), copied
  verbatim, only the lines porter names; a new one joins that file first.
- **The wire is serde.** Every stored or wire type has a round-trip test; enums with data are
  adjacently tagged (`kind`/`v`); the provider file is `ProviderSpec`'s serde form.
- **D-Bus signatures change with their XML.** `tests/introspection.rs` in porter-dbus fails
  until `dbus/*.xml` equals the skeletons' introspection; the failure prints the new text.
- **Refresh tokens, passwords and keys never cross a transport.** No wire type holds a
  `Credential`; apps receive `IssuedToken`.
- **A message carries no authority.** `prov::Message` is the one model for every exchange
  between agents: a request from the companion to a worker, a worker's or a computer-use run's
  progress and final report, the person's direct turn to a subagent (the person is a sender,
  `AgentRef::User`), a note, and a message across Spaces. A message is input. Nothing in it
  grants the receiver a permission, a budget or a memory read; a `Request` is evaluated under
  the receiver's own `TaskPolicy` and the router's gating pipeline as if the receiver had
  thought of it, and an untrusted sender's request is only a suggestion. Its label (the
  sender's taint included) travels and the receiver joins it into its own. A message may cross
  Spaces but keeps both ends' Spaces and never grants a memory read in the other. `from` is
  stamped by the transport from the caller (`Message::sender_matches`), never self-asserted.
- **The `desktop` scope is a sub-scope of every Space.** It holds only facts the person stated
  (`desktop_admits`); `Confidentiality::join` drops `desktop` when another Space is present, so
  reading a desktop fact into a `work` task leaves the task `Private({work})`.
- **Floats** appear only in `EmbedVector` (embeddings are floats end to end).

## 7. Who retries, who repairs

Decided with the rig amendment (the first appears in models.md section 4.2 once the spec is
edited; it is recorded here so the code and the spec agree):

- **inferd owns retry.** It sees the engine's state, so it retries a failed turn (a 5xx, a
  rate limit with its `Retry-After`, a not-ready engine that is loading) with stoker's
  `Retrying` wrapper and its pure `next_wait`, and never after the first event reached the
  client's sink: a half-delivered turn is not idempotent for the planner. A client sees either a
  reply or a `ModelError`; it never loops on `RateLimited` itself.
- **inferd owns validate-and-repair of structured output.** For `ReplyShape::Json` and
  `ReplyShape::Choice` it runs stoker's `ExtractSession` (`model-extract`): constrained decoding
  where the engine supports the shape, a synthetic tool call or a prompted schema where it does
  not, a bounded repair budget (the setting `ai.structured.repair_budget`, with `open_text`, `open_list` and `depth`: `[ai.structured]` of `inferd.toml`, read by `main` and passed down as `structured::Limits`; `config` therefore reads one type from `structured::limits`), and a check of the final text with `Shape::check`.
  A client (readerd, memoryd's consolidator, the action reviewer, the policy writer) gets either
  a `ChatReply.text` that already passes the check or `ModelError::Unparseable`; it parses into
  its type with a second, free check. A reply cut by `StopReason::MaxTokens` is never repaired.
  A repair prompt names the field and the expected shape and never echoes the model's output.
- **`ModelError::ContextOverflow` carries no limit.** The caller reads the context from the
  model's `LlmCap.context` (it has the card); stoker's `ProviderError::ContextOverflow { limit }`
  is mapped without it. `ProviderError::Server(status)` maps to `ModelError::Unreachable`
  (a gateway 5xx is a transient reach failure to an app).
- **Trace context.** `Inference1.Open`, `Prepare` and `Availability` take an `options` vardict
  whose reserved key is `traceparent` (`porter_dbus::OPTION_TRACEPARENT`, a
  `porter_infer::Traceparent`); `Accounts::session_with` and `Transport::open_with` carry it. No
  content ever goes on a span (`prov::trace`).

## 10. inferd hosted

Hosted models (`inferd::cloud`): stoker's curated remote entries are candidates only for an app
that holds a grant on an account reaching them. `cloud::accountd` asks `Peer.Verdicts` once per
session and `Peer.ResolveKey` once per turn (a sealed memfd, read into a `SecretText`);
`cloud::models` picks each entry's reach with stoker's `reachable` (a company's own account before
OpenRouter) and makes the card (locality cloud, the reach's price); `cloud::transport` is the TLS
`Transport` under stoker's `Driver<OpenAiCodec, _>`, `cloud::spend` the ledger and caps
(`ai.spend.*`), `cloud::turn` one turn, `cloud::picker` the `cloud/<model>` choices. The picker's `KeySpec.groups` puts each hosted choice under its company and each on-device model under "On this computer" (`settings::module`), "" and `auto` ungrouped on top. The key is in
no event, log, audit line or file; `tests/cloud.rs` scans every bus message with a positive control.

`org.quire.Inference1` at `/org/quire/Inference1`, claimed by `inferd` on the session bus
(`dist/dbus/org.quire.Inference1.service` activates `dist/inferd.service`; `dist/inferd.toml` is the
annotated sample configuration). One `Open` is:

1. **Who.** `peers`: the bus names the sender's pid, `/proc/<pid>/exe` names the program, the
   caller table names the `Caller` (an `AppId` and a `Role`). A sender the table does not name gets
   `org.freedesktop.DBus.Error.AccessDenied`. The role `Cua` (cuad's executable) is the only one
   that may open a computer-use session; any other caller's route is `Refused(Denied)`.
2. **Where.** A socketpair; one end goes back as the reply descriptor, `serve_session` runs on the
   other over this session's four seams:
   - `Router` is `engines::SessionRouter`: `Engines::route` lists the local models (the stoker
     catalog through `local`) with their readiness from the supervisor's snapshot, then
     `router::choose` filters by the need (`porter_core::matches`), readiness, consent (a model on
     this computer is granted; anything else asks) and `porter_infer::route` with the user's
     policy and tier map. The decision pins the runner through a `Pin` cell.
   - `EngineHost` is `Engines`: `want` asks the `Supervised` handle, which owns stoker's
     `Supervisor` and carries out its effects over `engine_supervisor::{EngineHost, ReadyProbe,
     GpuProbe}` (`ProcessHost`, `HealthProbe`, `NvidiaSmi` in the daemon; fakes in tests). Engines
     unload on the supervisor's idle timer, so `release` does nothing.
   - `TurnRunner` is `runner::Turns`: stoker's `OpenAiCompat` (`Driver<OpenAiCodec, HttpClient>`)
     over the engine's Unix socket, wrapped in `Retrying`; `bridge` maps both ways; computer-use
     steps are `cua_step` over stoker's `CuaSession`; a reply of a shape inferd can read is run
     under `structured` (a `ShapedSession`: validate, repair once); a `Transcribe` turn is
     `speech::SpeechRunner` over `SttBackend::SpeechHost` (stoker's `speech-host-client` to the supervised
     speech host, a CPU engine probed with the host protocol's `Hello`): the session's audio frames
     reach the turn through a channel and the host's events leave as `Heard` deltas.
   - `AuditSink` is `audit::SessionAudit` over an `AuditOut` (a JSON-lines file in the daemon);
     the loop tells it what the request carried (`serve::Carried`: frames sent, audio milliseconds
     sent or produced) beside the reply.
3. **Away.** The first event is `Waiting(readiness)` when the engine is not ready, then `Routed`,
   then the turn's events and one `Finished` per request, exactly as `session::step` decides.

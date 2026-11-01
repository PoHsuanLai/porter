# Changelog

## Unreleased

### Grants are on the bus

- `org.quire.Grants1` at `/org/quire/Grants1` on accountd's name (`dbus/org.quire.Grants1.xml`):
  `Candidates`, `Availability` and `Grants` for the caller's own grants, `RequestGrant` (a Request
  object, like `Keys.RequestKey`; the sheet is drawn each time it is asked for, and the person's
  answer is kept as a grant that `Candidates` and `Grants` show), `Revoke`, `OpenAuthenticated` and `OpenLinked` (a descriptor, `h`, a Unix stream
  socket whose other end accountd's relay holds, so no token or password crosses the bus),
  `Accounts` (every account with accountd's name for its provider, for a porter daemon, Settings
  or the shell), and `ExtendAccess` and `AsksOnUse` (the access a provider asks only on use).
  Apps and the porter daemons use it; every other role is `AccessDenied`.
  `RequestGrant` takes the one account to ask about (empty for none): the sheet then has no
  chooser and the answer is a grant for that account or a refusal, never another account. In
  `porter-client`, `ConsentOffer` has `account: Option<AccountId>` for it (`find` leaves it `None`);
  `AccountsRequest::Choose` carries it too.
  `porter_dbus::accounts::{GrantsProxy, GrantsServer, GrantsServed, to_text, from_text}`;
  `porter_wire::names::GRANTS_PATH`.
- `porter_client_dbus::Bus` implements `GrantsLink`. `GrantsLink` and `Grants` gain `accounts()`
  (`ListedAccount`: the account and its provider's name), `extend_access(account, kinds, window)`
  and `asks_on_use(provider, kind)`; `GrantsError` gains `Busy`. `porter-client-inproc`'s `Hosted`
  implements the three (`porter_service_grants::GrantsApi::accounts` is new).
- accountd runs the relays (`accountd::Relays`, `Options::relays`: the default trusts the
  platform's roots; a test passes a connector that trusts its scratch CA) and depends on
  `porter-proxy`.
- `porter_dbus::accounts::ServiceRoot::Sync` is gone, and with it the sync daemon's request root
  and error prefix: the sync daemon asks accountd for what it needs, and porter-sheets and
  porter-daemon name accountd alone.

### The sync daemon moved to the ferry repo

- `syncd`, `syncd-fakes`, `porter-sync` (now `ferry-sync`), `porter-drive` (`ferry-drive`),
  `porter-drive-fuse` (`ferry-drive-fuse`), `porter-mountd` (`ferry-mountd`; its binary, units and
  socket keep the name `porter-mountd`), `storage-webdav`, `storage-graph`, `storage-gdrive`,
  `quickxorhash` and `windows-zones` left porter for the ferry repo, which depends on porter by git
  rev. The units `syncd.service`, `porter-mountd.service` and `.socket`, the activation file
  `org.quire.Sync1.service`, `docs/drive-files.md` and the interface files `dbus/org.quire.Sync1.xml`
  and `dbus/org.quire.Storage1.xml` went with them, byte for byte; `scripts/install.sh` installs
  accountd only, and ferry's installs syncd and the helper.
- `porter_dbus::sync` and the `photos-picker` feature are gone: `Sync1`, `Storage1` and the Photos
  Picker are ferry's `ferry-bus`. `porter-dbus-macros`' `#[bus_interface]` names `::porter_dbus::Call`
  and `::porter_dbus::Unserved`, so a crate in another repo uses it by depending on `porter-dbus`.
- `porter-fake-servers` loses the Graph fake (`FakeGraph`, `GraphHandle`: it served the drive's
  `quickXorHash`); it is ferry's `ferry-fake-graph`. `porter-rig-servers` loses `--graph` and its
  `/graph/...` levers, `porter-rig-client` loses `sync-resolve`, `watch-conflicts`, `datasets` and
  `status`, and `rig.json` loses `graph`: ferry's `ferry-rig-servers` and `ferry-rig-client` do
  those, with the same lever routes and command words.
- `scripts/check-boundary.sh` fails if a manifest names `syncd`, a `storage-*` or a `ferry-*`
  crate, or the workspace's dependency trees reach one.

### The `NeedsReauth` signal is gone (wire change)

- `org.quire.Accounts1.Manager` no longer emits the `NeedsReauth(o)` signal. An account that needs
  signing in again is told by its `State` property (`PropertiesChanged` on the account object), which
  `porter_client::Accounts::watch` reads as `AccountChange::State`. `porter_dbus::accounts` loses the
  signal's `NeedsReauth` and `NeedsReauthStream` types and the `ManagerServed::needs_reauth` emitter.
  `needing_reauth()` and `Refusal::NeedsReauth` stay.

### Editing an account's servers (hosted apps)

- `porter_client::Accounts::edit_servers(EditServers, &ParentWindow) -> Result<EditedServers, EditError>`,
  over a transport that implements `ServerEdits` (the hosted one only; no bus method, no wire change).
  The sheet opens on the server form prefilled from the account, the servers the person types are
  logged in at (the incoming server, and the outgoing one when it, the login name or the password
  changed) before anything is handed back, and a refusal marks its field so the person sends again.
  Porter stores nothing: the app keeps the servers, the name and the new password in the
  `EditedServers` (`into_parts`, once; no `Clone`, no `PartialEq`, no public password field).
- `porter_sheet`: `Purpose::EditServers`, `Stage::Checking`, `FieldKind::Name` (asked only when the
  app names one with `EditServers::with_name`), `edit_form`, `servers_editable`, `servers_fixed`,
  `ServersFixed`, `EditServers`, `EditedServers`. `porter_provider::SignInMode::EditServers`; the
  generic family tries the typed servers with a password, never a bearer token.
- `porter_service_accounts::AccountsApi::edit_servers` and `EditError`. The add flow's `Job` is a trait
  whose `Done` is what its own entry point returns (`Add`, `Reauthenticate`, `Extend`, `EditServers`).
- `porter_client::AccountChange::NeedsReauth(AccountRef)` is gone: `AccountChange::State(AccountRef,
  AccountState)` says every change of an account's state, so signing in again and recovering are one
  path. `Bus::watch` hears the `State` property change of any account object and reads the state.

### The AI broker leaves porter for steward

- Deleted: `inferd`, `inferd-client`, `porter-infer`, `porter-router`, `porter-turns`, `porter-bridge`
  and `porter-tailnet`, with `porter_dbus::inference`, `dbus/org.quire.Inference1*.xml`, `dist/inferd*`,
  `dist/dbus/org.quire.Inference1.service` and `porter_discover::probe_ports`. They live in the steward
  repo, cut from e9e7cdcd; steward depends on porter by git rev, porter never on steward.
- `porter_wire::names::{INFERENCE_BUS, INFERENCE_PATH}` name steward's model service; accountd's
  old-key import reads them (they were `porter_dbus::inference::{INFERENCE_BUS, INFERENCE_PATH}`).
- `porter-rig-client open` is gone. `scripts/install.sh`, `check-boundary.sh`, `check-features.sh`,
  `check-portable.sh` and the CI test lines no longer name the moved crates.
- Workspace dependencies only they used are dropped: stoker's crates (`cua-action`, `model-*`,
  `engine-*`, `speech-*`, `vision-prep`, `cua-*`), `rust_decimal`, `iso_currency` and `getrandom`.
- `porter-dbus` no longer depends on `porter-match`; `porter-rig` neither.

### Engines go through stoker's `Engine` trait

- `porter-engines` and `porter-engines-install` are deleted. inferd and porter-router use stoker's
  `engine-spec`, `engine-manifest`, `engine-probe` and `engine-install` (stoker 80bf1239 and the
  commit that adds the trait methods below). The mirror enums are gone too: `porter_engines::EngineId`,
  inferd's `settings::missing::Engine` and `settings::programs::Program`. `model_catalog::EngineKind`
  is the one engine type.
- Words are unchanged: the bus ids (`llama_cpp`, `vllm`, `strata`, `speech`, `kokoro`), the running
  list's engine words (`vllm`, `llama-server`, `speech-host`, `kokoro`, `strata`), the `engines.*`
  settings keys, the install folders, the unit names and sockets (`llama_server:<model>`), the
  helper's arguments and each engine's command line and environment.
- The catalogue loader runs `engine_spec::check_entry` after the parser: a vLLM line with no GPU
  estimate or a llama-server line whose weights are not GGUF is skipped, as before, with the check's
  own message. `porter_router::catalog::parse_entry_text` returns `EntryError`.
- The privileged helper is stoker's `engine-helper` (it was `porter-engine-helper`), at
  `/usr/libexec/quire/engine-helper` under the polkit action `org.quire.engines.install`; stoker's
  `scripts/install.sh` installs both, and porter's no longer builds a helper or ships
  `dist/org.quire.porter.engines.policy` (stoker's install removes the old file). A caller of
  `engine_install::EngineInstalls` passes `HelperProgram { pkexec: "/usr/bin/pkexec", program:
  "/usr/libexec/quire/engine-helper" }`.
- `EngineConfig::program` is public and `EngineConfig::named_path` is new; `LocalModel::unit_with`
  returns an `Option`.

### Every domain of accountd is a client of `Porter<T>`

- `porter.identities()`, `spaces()`, `tailnet()`, `ai_keys()`, `ai_accounts()` and `launcher()` over `Bus`, and the
  shell's view of the accounts on `accounts()` (`needing_reauth`, `view`, `reauthenticate_shown`, `watch`, with
  `AccountRef`). They are the old `Identities`, `Spaces`, `Tailnet`, `AiKeys`, `AiAccounts` and `Launcher` without
  `connect(&BusConnection)`: they take a `Porter<Bus>` (`Bus::over(connection)`). `Spaces` gains `register`, `rename`,
  `set_look` and `remove`.
- Every watch is a `Changes<T>` (a `Stream` of `Result<T, Malformed>`, with `next`), replacing `SpaceChanges`,
  `MachineChanges`, `IdentityRemovals`, `Changes` (AI accounts), `Requests` and `Revocations`; an item is
  `Err(Malformed::Signal)` for a signal that is not well formed. `Spaces::watch` gives
  `Changes<(DesktopSpace, SpaceChange)>`; no domain has an alias.
- Errors carry no text: `CallFailure` and its three `String` variants are `LinkError`; each domain's error has
  `Link(LinkError)` where it had `Bus(CallFailure)`. A sheet already open for the app is `AccountsError::Busy`,
  where detent searched an error's text for `LimitsExceeded`; an object the daemon does not serve (an account that is
  not there) is `LinkError::Invalid`.
- `porter-client` reaches no zbus, no `porter-dbus` and no service crate, and the bus tests of the client are accountd's
  (`crates/accountd/tests/it/client`); check-boundary.sh has a table of the
  crates that may depend on `porter-dbus`, `porter-callers`, the `porter-service*` crates, `porter-identities`,
  `porter-spaces`, `porter-settings`, `porter-store` and `porter-sheets`, and fails on any other, and on
  `porter_dbus::accounts` named outside the daemons and `porter-client-dbus`.

### `Porter`: one handle over a transport

- `porter_client::Porter<T>` is the app's handle: `porter.accounts()` (`add`, `reauthenticate`) over any transport that
  signs an account in, and `porter.grants()` (`find`, `request_grant`, `grants`, `revoke`, `open_authenticated`,
  `open_linked`) over a transport that carries grants, which is the hosted one only. A transport implements
  `AccountsLink` and `GrantsLink`; a domain a transport does not carry has no accessor on it, so the
  `Unsupported`/`Unreachable` defaults are gone.
- The transports are crates of their own. `porter-client-dbus`: `Bus::over(connection)`, `Bus::accounts_at(owner)`,
  `connect()`. `porter-client-inproc`: `Hosted::new(service, app)`, `HostConfig` and `hosted(config)` for an app that
  brings its providers, secrets and sheet host, `RelayHost`, `NoRelays`, `PorterRelays` (feature `relays`).
- The agent launcher is `porter_client_dbus::Launcher` (`Bus::launcher()`), not a `Porter` domain: its process credential
  is a sealed memfd accountd hands over on the bus, a Linux descriptor, so `ProcessCredential`, `ChildKey`, `Delivery`,
  `Inherit` and `Revoked` live in `porter-client-dbus` and `porter-client` has no descriptor in its API and builds on
  macOS and Windows. `LauncherLink` is gone. `scripts/check-portable.sh` now cross-checks `porter-client`,
  `porter-sheet` and `porter-wire` on every macOS and Windows target rustup has installed.
- Errors are typed without prose: `LinkError` (`Unreachable`, `Closed`, `Denied`, `Invalid`, `Malformed(Malformed)`)
  beside each domain's own: `AccountsError` (`Refused`, `AlreadyAdded`, `Busy`, `Link`), `GrantsError`.
- Gone from `porter-client`: `Accounts<T>` (the god facade), `Transport`, `TransportError`, `ClientError`,
  `DbusTransport`, `InProcess`, `HostedCore`, `Relayed`, `Removals`, `RemovedAccount`, `Accounts::watch_removals`
  (nobody called it; `Identities::watch_removals` is the same signal) and the features `dbus`, `in-process`, `relays`.

### `porter_service::Service`

- `Service` is the facade seen as the two domain traits a host asks of it: `accounts_api()` (`AccountsApi`) and
  `grants_api()` (`GrantsApi`). `AccountService<..>` implements it; the client's hosted carrier is generic over it.

### Bus names, `LauncherFault` and `Destination` have one home each

- `ACCOUNTS_BUS`, `ACCOUNTS_PATH`, `ACCOUNTS_SETTINGS_PATH`, `SPACES_PATH`, `IDENTITIES_PATH`, `TAILNET_PATH`,
  `SHEET_BUS` and `SHEET_PATH` are `porter_wire::names::*` (they were `porter_dbus::accounts::*`), so an app
  finds the daemon without naming the bus crate. `LauncherFault` is `porter_wire::LauncherFault`; its error
  names are `porter_dbus::accounts::{launcher_fault_error_name, launcher_fault_error_name_in,
  launcher_fault_from_error_name}` (the methods `error_name`, `error_name_in`, `from_error_name` are gone).
- `Destination` (a call sent to a daemon's name or to the owner the caller checked) is `porter_dbus::Destination`,
  the one definition `porter-client` and `inferd-client` both use.

### The inference half of the client is `inferd-client`

- `porter-client` is the account service's client. What it held of inference moved, with
  history, to the new crate `inferd-client`, which sits in porter until the steward lane takes
  it: `Inference<T>` (the old `Accounts<T>`'s inference methods, `guests`, `candidates`, ...), the
  inference `Transport` (`open_with`, `prepare`, `places`, the computers and the guests),
  `DbusTransport` (inferd; `inference_at`), `Brokered<B>` (what `InProcess::with_broker` was),
  `TransportError`, `ClientError`, `ComputerReason`, `engines`, `TailnetLending`. Features
  `infer`, `engines` and `lending` left `porter-client`.
- `porter-client`'s `Transport` has `call`, `open_authenticated` and `open_linked` only;
  `InProcess<C, H>` has no broker type parameter (`NoBroker` is `inferd-client`'s);
  `DbusTransport::accounts_at` stays, `inference_at` is `inferd-client`'s. `ClientError` lost
  `InferRefused`.

### The sheet host seam is porter-sheet's

- `{Sheets, SheetLink, SheetOpen, SheetFault}` are `porter_sheet::host::*` (they were
  `porter_service_core::sheets::*`). `porter-sheet` depends on `porter-wire` for `ParentWindow`. A host
  that draws a sheet no longer names a service crate.

### AI keys are accountd's

- accountd keeps the AI keys, the agent logins and the local runtimes with the identities: one
  registry, one secret namespace (`porter.accountd`), one grant store, one audit file. inferd's
  second store (`keys/`, `launcher/`, the namespace `porter.inferd`, `INFERD_KEYS`, the
  `test-keys` feature and `inferd add`) is deleted. `accountd add` adds a key from a terminal;
  `ACCOUNTD_KEYS=file:<path>` reads the file format `INFERD_KEYS` did.
- Wire. `org.quire.Inference1.Keys` is `org.quire.Accounts1.Keys` (`RequestKey` unchanged;
  `AddAccount` is `Manager.AddAccount`). `org.quire.Inference1.Launcher` is
  `org.quire.Accounts1.Launcher` with the same members and signatures, minus `SetAgentState`;
  `IssueProcessCredential(permit, program) -> (id, h)` hands the key on a sealed memfd, the one
  way (the `target` argument and `Handoff` are gone, and so is the `handoff` field of the
  `process_credential_issued` audit line). The Request objects of key sheets are under
  `/org/quire/Accounts1/request`, errors under `org.quire.Accounts1.Error.*`, and
  `ServiceRoot::Inference` is gone. New: `org.quire.Accounts1.Broker`, for the new role
  `CallerRole::AiBroker` (`ai_broker`, inferd's row in `callers.toml`): `Verdicts`, `ReadKey`,
  `Accounts`, `Servers`, `ReadServerKey`, `ReportLocal` and `Changed`. The AI account rows move
  from `/org/quire/Inference1/settings` to accountd's settings module; only
  `accounts.<id>.spend.month` and `.spend.cap` stay with inferd.
- porter-client. `AiKeys` and `Launcher` talk to accountd; `AiKeys::add_account` and the `target`
  of `Launcher::issue_credential` are gone, `CredentialHandle` with them (`ProcessCredential`
  holds the memfd: `descriptor()`, `read_key()`, `child_key()`). New `AiAccounts` for the AI
  broker.
- Data. accountd copies inferd's registry, keys, permits, switches and audit lines in at its
  start, reads both stores back before it removes the old keys, and puts the copy off while an
  inferd that still keeps its own keys is on the bus. inferd removes its old registry and audit
  file once accountd's record (`ai-import.json`) covers them. The registry is no longer pruned to
  identities, and `registry.pre-split.json` is no longer made.
- Sandboxes. No limit changes: accountd's unit has no new directory, and inferd's loses
  `inferd/agent` from `RuntimeDirectory`.

### Capability kinds are written once

- `capability_kinds!` in porter-capability is the one list of kinds. `Capability`,
  `CapabilityKind` (with `ALL`, `name` and `is_ai`), porter-match's `Need`, the dispatch that
  meets a need with an offer and the `Service` type of each kind (`porter_capability::service::*`,
  with `Asks` in porter-match for its need) are expanded from it; a new kind is one row, its
  capability struct and its need struct. Each need says how it meets its capability by
  `Wants<C>::short_of`, and `Shortfall` is unchanged. The serde forms are unchanged and pinned by
  a test. `porter_sheet::words::service_name(kind)` is `kind.name()`.

### Settings rows are written once

- New crate porter-settings: `AccountRow` and one unit struct per row every account has, which
  accountd and inferd list their accounts through in place of two copies of the same eight
  `Key` variants. accountd's keys (`Key`, `parse`, `path`, `schema`) and the edit of
  `clients.toml` moved there from accountd; the daemon hosts the module. The row paths and words
  are unchanged.
- The rows that carry an id or an index (`used_by.<n>`, `service.<kind>`, `grant.<grant>` and its
  four read-outs, `sign_out`, `expires`) are `AccountRow` impls too, found by the path under the
  account's id, and a permit's five rows are `GrantRow` impls: `Key` is an account with its row
  and path, or an issuer's client row, and the daemon does what an action row asks through
  `Acts`. inferd's two spend rows are `SpendRow` impls. Adding a row is one impl and one line of
  `ROWS`. Every key path, word and value is unchanged; a table test holds them.

### The service is a facade over domains

- porter-service is split. New crates: porter-service-core (`Ctx<E: Ports>`, the registry, the
  audit trail, the sheets seam, the reach policy, the per-account turns at an issuer),
  porter-service-accounts (`AccountsApi`), porter-service-grants (`GrantsApi`), porter-identities
  (`UseBook`, `Picks`, the `IdentityDesk`) and porter-spaces (`SpaceBook`), the last two taken out of
  accountd. `porter_service` keeps `AccountService`, which a host builds as before. `Ports` names
  the six seams once, so the domains' impls carry one parameter in place of six; `Parts<P, S, U, K,
  R, A>` is the `Ports` a host builds, and `AccountService` is still generic over the six.
- Moved: `porter_service::{Registry}` is `porter_service_core::registry::Registry`;
  `{audit::*, AuditSink, NoAudit}` are `porter_service_core::audit::*`; `{Sheets, SheetLink,
  SheetOpen, SheetFault}` are `porter_service_core::sheets::*`; `{SignInAs, AllowFor, AgentFault,
  Launchers, LoginEnd, NoLauncher, Roster, Waiting, program_of, LocalFault, MAX_LOCAL_CLAIMS,
  OwnMachines}` are `porter_service_accounts::*`; `accountd::{IdentityDesk, SheetsDesk}` are
  `porter_identities::*` and `accountd::{CREATES_PER_WINDOW, CREATE_WINDOW}` are
  `porter_spaces::*`.
- `LauncherSession` is `SessionToken`, with the same text on the wire and in `registry.json`:
  consent only compares tokens and no longer names the agent launcher. `choose_for_agent` is
  `AccountService::choose`, which takes the session.

### The wire carries only sheets

- `AccountsRequest` is `Choose`, `AddAccount` and `Reauthenticate`, and `AccountsReply` is
  `Chosen`, `Added`, `AlreadyAdded`, `Reauthenticated` and `Refused`. Deleted, because no daemon
  served them: the requests `Query`, `Availability`, `ListGrants`, `Revoke` and `IssueToken`, and
  the replies `Candidates`, `Availability`, `Grants`, `Revoked` and `Token`. What an app read of
  its grants is typed on the service: `AccountService::{candidates, availability, grants,
  revoke}`. `porter_client::Accounts::find` reads it through the in-process transport and is
  offered only there (`Accounts<InProcess<..>>`); over the bus it was already unsupported.
  Nothing called `IssueToken` outside tests, so the token it issued is gone with it
  (`AccountService` has no `issue_token`, `porter_client::Accounts` no `token`), and so are the
  client's `grants` and `revoke`. `AuditEvent::TokenIssued` is read and never written: an old
  `audit.jsonl` line of that kind still reads and shows. `Choose` stays: inferd's `Keys` sheet
  and syncd's own grants use it, and accountd will serve it.

### Sans-IO is checked

- check-boundary fails when a `*-proto` crate depends on porter-http, tokio or reqwest, or has an
  `async fn`. No crate failed it.

### porter-families is the registry, and each family is a crate

- New crates porter-family-kit, porter-family-nextcloud, porter-family-generic and
  porter-family-oauth. porter-families keeps `DynProvider`, `set` and the small families, and its
  `nextcloud`, `generic` and `oauth` features turn the crates on. The paths moved:
  `porter_families::{Conversation, Flow, Move, Pacing}` are `porter_family_kit::..`;
  `{NextcloudProvider, NextcloudFlow, NextcloudSession}` are `porter_family_nextcloud::..`;
  `{GenericProvider, GenericFlow, GenericSession, MailLookup, DavLookup, FixedLookup, Lookup,
  Located, Mid, Outcome, Seams, SharedDns}` are `porter_family_generic::..`; `{OAuthProvider,
  OAuthSession, OAuthEnv, BrowserFlow, BrowserIssuer, Browser, Cx, Grant, Standard, Google,
  Microsoft, GoogleState, ClientFiles, Clock, Random}` are `porter_family_oauth::..`.

### The OAuth probes

- New crate oauth-probe-proto: the bearer read and the readings of Google's and Graph's answers as
  claims. Which path shows which service is a `[[<issuer>.probe]]` row of `issuers.toml`
  (`IssuerData::probe`, `ProbeRow`). `porter_families::is_personal` is
  `oauth_probe_proto::microsoft::is_personal`.

### generic-proto

- New crate generic-proto: the typed and file-named mail servers (`typed`, `fixed`, `domain_of`),
  the `.well-known` and PROPFIND requests of DAV discovery with their verdicts, the JMAP session
  request and the plan of the password check, with no IO. The generic family only sends them.

### nextcloud-proto

- New crate nextcloud-proto: Login Flow v2, the OCS capabilities (moved from porter-discover:
  `parse_ocs_capabilities` is `nextcloud_proto::ocs::parse_capabilities` and returns the claims,
  and `Source::Ocs` is gone), the app-password revoke, the paths of a server and the account's
  endpoints, as requests and parsers. The Nextcloud family only sends them. porter-core gains
  `EndpointUrl::join`, `EndpointUrl::typed_server` and `ServiceEndpoint::coherent`,
  porter-capability `offers`; the families' own copies are gone.

### One DAV discovery, and porter-dav is gone

- dav-proto: feature `discover` adds `discover::Walk`, the one sans-IO state machine for the
  principal, home-set and collection walk (`start` returns the walk and its first request,
  `advance` takes the reply and returns `Next::Ask` or `Next::Done(Found)`), with `Reach`,
  `Scope`, `Home`, `Collection`, `Found`, `Error` and the readers `current_user_principal`,
  `home_set` and `collections`. `quota` and `Quota` move in from porter-dav. `http::bytes` builds
  the requests as `Request<Vec<u8>>`. `names::CALENDAR_COLOR`, `Multistatus::first_found`,
  `first_found_items`, `Response::found_items` and `Prop::items` are new. Nothing is removed.
- A home-set or principal href with a literal space in it is one href: it was split on the space.
- porter-dav is deleted. Its `propfind` and `report_sync_collection` are `dav_proto::http::bytes`
  (with `porter_http::uri` for a `WebUrl`), and its readers are dav-proto's. The Nextcloud and
  generic families and syncd's PIM discovery all drive `Walk`.

### Families are traits, and OAuth is one family

- porter-families: `FamilyProvider`, `FamilySession` and `FamilySignIn` are gone. A family is held
  as `Box<dyn DynProvider>` (`DynProvider::boxed(provider)`), which is a `Provider`; `DynSession`
  and `DynSignIn` are its session and sign-in. `set::family_of` walks a table of builders.
- The `google` and `microsoft` features are one `oauth` feature. `GoogleProvider`,
  `MicrosoftProvider`, `GoogleEnv`, `MicrosoftEnv`, `GoogleSession`, `MicrosoftSession`,
  `GoogleSignIn` and `MicrosoftSignIn` are `OAuthProvider<Google | Microsoft>`, `OAuthEnv`,
  `OAuthSession<..>` and `Conversation<BrowserFlow<..>>`. An issuer whose row in
  porter-provider's `issuers.toml` names a userinfo endpoint is served by `Standard` with no Rust.
- porter-families: the sign-in types of the other families are `Conversation<NextcloudFlow>`,
  `Conversation<ApiKeyFlow<H>>`, `Conversation<AgentLoginFlow>`, `Conversation<TailnetFlow>` and
  `Conversation<GenericFlow<L>>`; `GenericProvider<L>` takes the `Lookup` (`MailLookup`,
  `DavLookup`, `FixedLookup`) its discovery names.
- porter-provider: `Issuer::Yahoo`; `Issuer::data()` (endpoints, userinfo, scopes by capability
  kind, scope classes, API origins) replaces the endpoint match, and `scopes_of`,
  `on_use_scopes_of`, `scope_sensitivity` and `Sensitivity` of porter-families are
  `IssuerData::{kind_scopes, on_use_scopes, class_of}` and `ScopeClass`. Yahoo is an issuer row
  (endpoints and userinfo); a Yahoo provider file comes once its scopes are confirmed against a
  registered client.

### Dead API, no-op migrations, the one-batch shims and the socket carrier are deleted

- porter-oauth: the device-code flow, `mint_key`, `openrouter_auth_url`, `clients_toml`,
  `MovedEndpoints` and `ClientRegistry::moved_endpoints` are gone, with the fake issuer's device
  grant. porter-families has no `openrouter` feature or skeleton family. `AuditEvent::Adopted`
  and `porter_sync::DatasetKind::{Settings, Keychain}` are gone; an `audit.jsonl` line written
  for `adopted` is skipped on read, like any line of a kind this build does not know.
- The registry's ten version-only migrations are gone: a vocabulary that changed no stored
  shape has no row in the table, and a document from any stored version still reads.
- Every re-export "kept for one batch" is deleted, and each caller names the real path:
  `porter_core::clock::{Clock, FixedClock, SystemClock}` replace `porter_service::Clock`,
  `porter_fake::FixedClock` and `syncd::clock`'s; porter-dbus's `names`, `callers` and `sheet`
  items are named through those modules, not at the crate root; `porter_http::{Method, Status,
  HeaderName, HeaderValue}`, `porter_dav`'s dav-proto types, `storage_webdav`, `storage_graph`
  and `storage_gdrive`'s `Dial`, `StreamHttp` and `StreamLimits`, `porter_tailnet::{GuestRow,
  RowState}`, porter-rig's `secrets_service` and `storage_fake`, accountd's and syncd's
  `proc_root`, and porter-fake's `infer` feature with its `FakeModel`, `FakeInferSession`,
  `Script` and `ScriptStep` (use `porter_infer::testing`) are gone.
- porter-client has no default features: name `infer`, `in-process`, `dbus` and the rest as
  needed. The latchkey socket carrier is deleted: `SocketTransport`, `SocketSession`,
  `SocketAgent`, `StartAgent`, `Place`, `START_WAIT`, `LinkChoice::Socket`, the `socket` and
  `framed` features and the `latchkey` dependency, with porter-infer's `LinkHello` and
  `OpenFrame`. `ClientEnv`, `LinkChoice`, `AnyTransport` and `AnySession` are gone with it:
  `Accounts::<DbusTransport>::connect()` connects over the session bus (feature `dbus`), and
  callers name `DbusTransport` and `DbusSession`.
- `IssuerEndpoints::device` is gone: nothing reads a device endpoint now.

### porter-http speaks the `http` crate's request and response

- `HttpRequest` and `HttpResponse` are now `http::Request<Vec<u8>>` and `http::Response<Vec<u8>>`
  (http 1.x, re-exported as `porter_http::http`). The `Http` trait, `SharedHttp`, `HyperHttp` and
  `stream::StreamHttp` take and return them. The body stays a whole `Vec<u8>`: what porter sends
  and reads is small, and every caller reads it as a slice or a string.
- porter-http's own `Method`, `Status`, `Header`, `HeaderName` and `HeaderValue` are gone. Use
  `http::Method` (`Method::GET`; `porter_http::webdav::{propfind, report, mkcol}` for WebDAV's),
  `http::StatusCode`, and `http::HeaderMap`. `porter_http::{Method, Status, HeaderName,
  HeaderValue}` re-export the `http` crate's types for one batch; `Header` has no re-export.
- A request is built with `porter_http::request(method, &web_url)`, the one place a `WebUrl`
  becomes the request's URI, which returns `http::request::Builder`; `request_to` does the same
  for an `EndpointUrl` (it replaces `HttpRequest::to`). A header value the `http` crate refuses
  (a line break in a token) now fails when the request is built; `HttpError` converts from
  `http::Error` as `Malformed`, which is what the hyper client answered for it before.
- `HeaderText` reads a header as text (`header(name)`) and `retry_after_seconds()` on a request,
  a response or a `HeaderMap`; it replaces `HttpResponse::header` and
  `HttpResponse::retry_after_seconds`. `SensitiveHeader::sensitive_header` marks a credential's
  value sensitive, so a request's `Debug` shows `Sensitive` for it. A request's `Debug` is now
  the `http` crate's and shows the whole URI, not only its origin.
- `HyperHttp` checks the rule that plain `http` goes to this computer only on the request's URI
  itself, since a request no longer has to come from a `WebUrl`. `BodySink::head` takes
  `(StatusCode, &HeaderMap)` and `HyperHttp::stream` returns `StatusCode`.
- `porter_dav::propfind` and `porter_dav::report_sync_collection` return
  `Result<HttpRequest, HttpError>`: building the request can fail on a URL the `http` crate does
  not take.

### WebDAV as a library: dav-proto

- New crate `dav-proto`: WebDAV, CalDAV and CardDAV bodies built as `String` and replies read
  from `&str`, with no porter type and no IO. It holds porter-dav's model (`Prop`, `PropStatus`,
  `Response`, `Multistatus`, `Depth`, `names`, the multistatus and sync-collection parsers) and
  what only mailo had (`multiget`, the typed `carddav::Props`); feature `http` builds the
  requests as `http::Request<String>`.
- A sync-collection report the server cut short (a 507 response, RFC 6578 section 3.6) is
  `SyncEnd::Truncated` with its token, no longer read as an expired token. storage-webdav ends
  that page `More::More` at the token, so the engine continues instead of listing from the
  start again (which on a big tree looped forever). The PIM `DavFeed` gets the same through the
  replica. A whole-request 507 is no longer `token_expired`.
- porter-dav keeps the `HttpRequest` builders and the discovery and quota readers. Its old
  paths for the moved items re-export dav-proto for one batch (`#[doc(hidden)]`);
  `porter_dav::DavFault` is `dav_proto::Error` (`Xml(String)`, `NotMultistatus`).
- Changed: `parse_sync_collection` returns `SyncReply` (no `Result`), and `SyncReply.sync_token`
  is `SyncReply.end` (`token()` reads it). `report_sync_collection` takes a `SyncLevel` and the
  properties; storage-webdav builds its report with it instead of its own copy.

### Test crates

- `FakeModel`, `FakeInferSession`, `Script` and `ScriptStep` moved from porter-fake to
  `porter_infer::testing`, behind porter-infer's new `testing` feature. porter-fake re-exports
  them under its default feature `infer` for one batch; porter's own crates turn it off, so
  porter-core's tests no longer build porter-infer.
- porter-rig's fake Secret Service is its own crate, `fake-secret-service`: `serve(&Connection)`
  returns its own `Error`, and the wire types (`Secret`, `SecretError`, the objects) are private.
  `porter_rig::secrets_service` re-exports the crate for one batch.
- The fake `org.quire.Storage1` and its binary `porter-rig-storage` moved from porter-rig to a
  new test crate, `syncd-fakes` (`syncd_fakes::storage`). The binary keeps its name and flags;
  build it with `cargo build -p syncd-fakes`. `porter_rig::storage_fake` re-exports the module
  for one batch.

### Keyring chunking is its own crate

- `keyring-chunked` is a new library crate: `ChunkedStore<S>`, a keyring-core store over any
  other that keeps a value too long for one entry (the Windows Credential Manager's 2560 bytes) in
  parts, with its own `ChunkError`. porter-secrets' `StoreSecrets` uses it, and its private
  `keyring/chunks.rs` is gone. What porter wrote reads unchanged: the head `porter-parts:<count>`
  and the parts `<name>#1`, `<name>#2`, ...
- A write that fails part-way now removes what it overwrote, so the credential reads as missing
  instead of as two values mixed; one that fails before any part is written leaves the old value.
- Parts left past the new count by an earlier failed write are swept on the next write or delete.
- A value that would need more than 64 parts is refused when it is written (`TooLong`), not when
  it is read back.

### Engine archives are unpacked in process

- porter-engines-install unpacks a pinned `tar.gz` with the `tar` and `flate2` crates instead of
  running `/usr/bin/tar`. An entry with an absolute path or a `..`, a path through a link, a link
  that points out of the install, and a device or pipe are refused (`ExtractError::Refused`);
  before, `tar` alone decided. The free-space check, the seal check and the final rename are as
  they were.
- `ExtractError::NoTar` is gone: a computer without `tar` installs engines too.

### The agent token uses getrandom and subtle

- inferd's agent token takes its bytes from `getrandom` instead of reading `/dev/urandom`, and is
  compared with `subtle`'s constant-time compare instead of a hand-written loop. Both are in the
  workspace dependency block.

### Routing reads a permit, not a consent grant

- `porter_infer::RouteCandidate::permission` and `porter_router::Listed::permission` are a
  `porter_infer::Permit` (`Granted`, `Askable`, `Denied`), no longer a `porter_core::consent::Verdict`.
  inferd maps the verdict accountd gave a hosted model at its edge (`RemoteModel::permit`).
- The made-up grants are gone: a model on this computer, an attached engine and an engine the app
  configured in porter-client are `Permit::Granted` instead of carrying a grant named
  `on-this-computer`, `attached-engine` or `in-process`.
- `porter_infer::Broker`, which refused every request, is gone. inferd serves requests through its
  own pipeline, as before.

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

### porter-core holds less

- QuickXorHash is its own crate, `quickxorhash`: a RustCrypto `digest` hasher of 20 bytes, checked
  against Microsoft's reference outputs, with no porter type in it. `porter_core::quick_xor` is
  gone; nothing outside porter named it.
- The XDG base directories (`Xdg`, `PathError`) are `porter_daemon::xdg`, beside the other pieces the
  daemons share; `porter_core::xdg` is gone (only accountd, syncd and inferd named it).
- What accountd audits (`AuditEntry`, `AuditEvent`, `IssuerSlug`, `ClientIdChange`) is
  `porter_service::audit`, beside the `AuditSink` that receives it; the module is public now.
  porter-core keeps `Handoff` and `CredentialEnd` in `porter_core::audit`, the words a bus
  client names without the account service, and `porter_service::audit` re-exports them. No
  re-export is left at the old path: porter-core cannot depend on porter-service, and nothing
  outside porter named the moved types.

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

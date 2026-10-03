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
| `prov` | provenance for the companion: the shared ids (`SessionId`, `RunId`, `TaskId`, `EntityId`, `ActionName`, `MessageId`, `ThreadId`), `Effect`, the merged `Actor` (`AgentRole::Worker` beside `Cua`), the one `Message` model with `AgentRef` and `Address`, the desktop-scope join (`Confidentiality::join`, `flow_to`, `desktop_admits`), the label lattice (`Label`, `Labelled`, `Quarantined`) and the confirmation `Witness`; re-exports `SpaceId`, `SpaceScope`, `AppName`, `DataClass`, `UnixSeconds` | none |
| `porter-core` | the vocabulary: ids and `SpaceId`/`SpaceScope`, `Account`, the capability vocabulary (`Capability`, `CapabilityKind`, computer use included), `Need` and `matches`, provenance and `effective`, `Restriction`, `Locality`/`Tier`/`Billing`, `DataClass`, consent (`Grant`, `decide`, `availability`, the sheet's ask and answer), `Credential`/`SecretKey`, `IssuedToken`, the wire protocol (`AccountsRequest`, `AccountsReply`, frames) | none |
| `porter-provider` | provider files (`ProviderSpec`, `parse_provider`), `ProviderSet`, `Family`, `Issuer`, the `Provider` and `ProviderSession` traits | none |
| `porter-secrets` | the `Secrets` trait, the Secret Service attribute scheme, `MemorySecrets` (feature `testing`), `Oo7Secrets` (feature `oo7`, stubbed) | none today; oo7 behind its feature |
| `porter-sync` | the sync contract: `Replica`, `Cursor`/`Anchor`, `BaseVersion`, `Change`/`Tombstone`, `Conflict`, `DatasetKind`; `MemoryReplica` (feature `testing`) | none |
| `porter-infer` | the AI broker's pure half: requests and replies (chat with tools and controls, embeddings with a query/document role, tasks, computer-use steps, speech), `OpenOptions` (the reserved `traceparent`), the streaming session (`ClientFrame`, `InferEvent`, `InferSession`), the model picker's data (`AiKind`, `TierMap`, `PickerRow`), `Readiness`, `Policy` and floors, `route`, spend caps, `AuditEntry`, the `Model` trait, `Broker` (stubbed) | none |
| `porter-service` | accountd's core over its seams: `AccountService`, `Registry`, the `Prompter` and `Clock` traits | none (seams are passed in) |
| `porter-client` | the app-facing API: `Accounts`, `Found`, the `Transport` trait (`call` for accountd, `open` for an inference session); `InProcess`, `SocketTransport`, `DbusTransport` (feature `dbus`) | through its transport |
| `porter-dbus` | `org.quire.Accounts1`, `org.quire.Sync1`, `org.quire.Inference1` as zbus proxies and skeletons; `introspection`; the argument codec (stubbed) | zbus |
| `porter-fake` | test-only: fake providers from real provider files, three accounts, `ScriptedPrompter`, `FixedClock`, `FakeModel` (streams), `FakeInferSession` (scripted events per request kind, audio-gated transcripts), `fake_service` | none |
| `accountd`, `syncd`, `inferd` | the daemons. `accountd` and `syncd` are skeletons that build their service over the seams and exit with "not implemented"; `inferd` serves `org.quire.Inference1` (section 10). `inferd` is also a library (`adapters`, `bridge`, `session`, `serve`, `service`, `peers`, `router`, `engines`, `supervise`, `hosts`, `local`, `catalog`, `config`, `runner`, `cua_step`, `cua_run`, `audit`, `clock`, `speech`) so its modules are tested without a bus | everything |

Allowed direct edges (checked by `scripts/check-boundary.sh`; dev-dependencies are outside it):

| Crate | May depend on |
| --- | --- |
| `porter-core` | nothing of ours |
| `prov`, `porter-provider`, `porter-secrets`, `porter-sync`, `porter-dbus` | `porter-core` |
| `porter-infer` | `porter-core`, `cua-action` (stoker's computer-use vocabulary, by sibling path) |
| `porter-service` | `porter-core`, `porter-provider`, `porter-secrets` |
| `porter-client` | `porter-core`, `porter-infer`, `porter-provider`, `porter-secrets`, `porter-service`; `porter-dbus` with feature `dbus` |
| `porter-fake` | `porter-core`, `porter-infer`, `porter-provider`, `porter-secrets`, `porter-service` |
| `accountd` | `porter-core`, `porter-dbus`, `porter-provider`, `porter-secrets`, `porter-service` |
| `syncd` | `porter-dbus`, `porter-sync` |
| `inferd` | `porter-core`, `porter-dbus`, `porter-infer`, `cua-action`, and stoker's `model-provider`, `model-catalog`, `engine-supervisor`, `model-http` (feature `hyper`), `model-openai-compat`, `vision-prep` (feature `pixels`), `cua-parse`, `speech-provider`, by sibling path |

External boundaries: every crate but `porter-dbus` and the daemons never reaches `zbus`,
`zvariant`, `tokio`, `reqwest`, `hyper`, `ureq`, `oo7`, `keyring`, `secret-service`,
`interprocess` or `latchkey` (default features); `porter-core` also never reaches `toml`.
`porter-dbus` reaches `tokio` only through zbus's `tokio` feature (the pinned block's zbus line).
`tokio` is a direct dependency of the daemons only, and a dev-dependency of async tests.

## 2. Modules

| Crate | Modules |
| --- | --- |
| `prov` | `ids`, `effect`, `actor`, `agent`, `label`, `scope`, `message`, `consent`, `trace` (span attribute names, `slug`s) |
| `porter-core` | `id`, `app_id`, `units`, `space`, `error` < `capability` (`terms`, `mail`, `pim`, `storage`, `photos`, `ai`, `sync_kinds`, `kind`) < `need` (`data`, `ai`) < `offer`, `effective`, `matching`, `restriction`, `ai_props`, `data_class`, `auth_kind`, `account` < `consent` (`grant`, `decide`, `prompt`) < `secret`, `token`, `candidate` < `wire` (`request`, `reply`, `frame`) |
| `porter-provider` | `family`, `error` < `spec` (`auth`, `discovery`) < `parse`, `set` < `provider` |
| `porter-secrets` | `error`, `attributes` < `secrets` < `memory`, `oo7` |
| `porter-sync` | `anchor`, `item`, `transfer` < `change`, `refusal`, `dataset` < `replica` < `memory` |
| `porter-infer` | `ids`, `control`, `open`, `request`, `cua`, `speech`, `reply`, `error`, `readiness` < `event`, `session`, `choice` < `policy`, `spend`, `audit` < `route`, `model` < `broker` |
| `porter-service` | `clock`, `prompter` < `registry` < `choose`, `token` < `service` |
| `porter-client` | `error`, `env`, `found` < `transport` (`in_process`, `socket`, `dbus`; each with its session) < `accounts` |
| `porter-dbus` | `names`, `args` < `codec` < `manager`, `account`, `grants`, `tokens`, `request`, `sync`, `inference` < `introspect` |
| `inferd` | `session` (pure machine) < `serve` (the loop over four seams) ; `catalog` < `local` < `router` < `supervise`, `hosts` < `engines` (`Engines`, `SessionRouter`) ; `bridge` (`request`, `reply`) < `cua_step`, `runner` ; `audit`, `clock`, `peers`, `config` < `service` (the bus object) < `main` |

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
| which model answers a session, and what it is pinned to | `inferd::router::choose` over `inferd::engines::Engines::route` (the only caller of `porter_infer::route`) |
| the models this computer can run, from the stoker catalog | `inferd::catalog` (claims) and `inferd::local` (the book) |
| starting, probing and unloading an engine | `inferd::supervise` (the driver of stoker's `step`) over `inferd::hosts` |
| porter's request to stoker's turn, and back | `inferd::bridge` (the only mapping) |
| provider file format | `porter-provider::spec` + `parse` |
| which secret an auth kind presents | `porter-service::secret_purpose` |
| the account registry and candidates | `porter-service::registry` |
| the chooser/consent flow | `porter-service::choose` |
| the sync contract | `porter-sync::replica` |
| AI routing | `porter-infer::route` (the only place) |
| spend arithmetic | `porter-infer::spend` |
| D-Bus names and paths | `porter-dbus::names` |
| D-Bus argument shapes | `porter-dbus::args`, conversions in `porter-dbus::codec` |
| `Found` for apps | `porter-client::found` |
| the system clock | `accountd`'s `clock.rs` (the only reader of the wall clock) |

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

// porter-service: accounts-ui (accountd) or ScriptedPrompter; the system or a fixed clock.
pub trait Prompter: Send + Sync {
    fn ask(&self, ask: ConsentAsk, window: &ParentWindow) -> impl Future<Output = ConsentAnswer> + Send;
}
pub trait Clock: Send + Sync { fn now(&self) -> UnixSeconds; }

// porter-client: D-Bus, the latchkey socket, in process.
pub trait Transport: Send + Sync {
    type Session: InferSession;
    fn call(&self, request: AccountsRequest) -> impl Future<Output = Result<AccountsReply, TransportError>> + Send;
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
`AuthKind`, `Family`, `Issuer`, `Discovery`, `DataClass`, `Provenance`, `AbsentReason`,
`Locality`, `SecretPurpose`, `AccountsRequest`/`AccountsReply`/`Refusal`,
`InferRequest`/`ClientFrame`/`InferEvent`/`InferReply`/`InferRefusal`, `AiKind`, `Readiness`, `DatasetKind`, `Found`.

## 5. What is frozen, what is built, what is stubbed

Frozen means: the types, trait signatures, wire and file formats and D-Bus signatures below are
the interface other work builds on; a change is a vocabulary bump (section 6) or a design/31 edit.

| Piece | State |
| --- | --- |
| capability vocabulary, needs, `matches`, `effective`, restrictions, AI properties | built, table-tested |
| consent: `decide`, `availability`, the sheet's ask/answer | built, table-tested |
| ids, `AppName`, `LanguageTag` parsing; credential redaction | built, tested |
| wire enums and socket frames | built, round-trip tested |
| provider file format, parser and checks, `ProviderSet` | built, tested; four shipped files in `providers/` (`local.toml` is the supervised engines) |
| `Secrets` trait, attributes, `MemorySecrets` | built, tested |
| `Oo7Secrets` | stub (`todo!()`) |
| sync contract and `MemoryReplica` | built, contract-tested |
| routing, floors, spend arithmetic | built, table-tested |
| `Broker::infer` (streaming into a `ChatSink`) | stub |
| `prov` ids, `Effect`, `Actor`, `Quarantined`, `ReaderKey`, `Labelled::map` | built, round-trip and redaction tested |
| `prov::{Message, AgentRef, Address}`, `Message::check`, `AgentRef::of`, `Confidentiality::{join, flow_to}`, `desktop_admits` | built, round-trip, pinned-JSON and table tested |
| `prov` lattice: `Label::join`, `trusted_user`, `untrusted`, `Labelled::zip`, `endorse`, `declassify` | stub |
| porter-core: `SpaceId`, `SpaceScope`, `GrantKey.space`, `Grant<K>`, `decide<K>`, computer use, `DataClass::Voice`, `VocabVersion(2)` | built, table-tested |
| porter-infer wire: tools, images, `ChatControl`, `StopReason`, `ThoughtPart`, `ReplyShape::Choice`, `EmbedRole`, `CuaBegin`/`CuaStep`, `Transcribe`/`Speak`, `ClientFrame`, `InferEvent`, replies | built, round-trip and pinned-JSON tested |
| `OpenOptions`, `Traceparent`, the `options` dictionary of `Inference1.Open`/`Prepare`/`Availability` | built (inferd reads `traceparent` and ignores unknown keys; it writes no spans yet) |
| `prov::trace` names, `ActorKind::slug`, `Effect::slug` | built, tested against the serde forms |
| `tier_choice`, `AiKind::setting_key`, `InferRequest::kind` | built, table-tested |
| `picker_rows` | stub |
| `inferd::session::step` | built (Routed/Waiting events, audio effects, cua progress, one queued request); `fits` built |
| `inferd::serve` (`serve_session` over `Router`, `EngineHost`, `TurnRunner`, `AuditSink`) | built, tested over scripted seams |
| `inferd::service` (`Inference1`: `Open` with the caller check, `Availability`, `Prepare`, `Usage`, `Rescan`, `EnginesChanged`, `Gpu`), `peers`, `config`, `main` | built; tested on a private bus with fake engines (`tests/hosted.rs`); `Usage` answers an empty dictionary (nothing meters) |
| the real seams: `router` and `engines::SessionRouter`, `engines::Engines` over `supervise` and `hosts`, `runner::Turns` over `bridge` and `cua_step`, `audit` | built; the engine host is child processes (`ProcessHost`), not systemd transient units |
| `inferd::speech` rules (`check_audio`, `audio_ms`) | built, table-tested; `SpeechRunner` stub |
| `inferd::{catalog, local, bridge}` (the catalog read, its claims, the model book, both halves of the mapping to stoker's turns) | built, table-tested |
| `inferd::cua_run::CuaRun::step` | stub (its signature cannot reach the engine or the frame; `cua_step` does the work meanwhile, see FINDINGS "Fill F3: inferd") |
| `FakeInferSession`, `FakeModel` streaming | built, tested |
| `AccountService`: Query, Availability, Choose, ListGrants, Revoke, IssueToken, `remove_account` | built over the seams, tested end to end with the fakes |
| `AccountService`: AddAccount, Reauthenticate | stub |
| `Accounts` (client API), `found`, `InProcess` accounts calls | built, tested end to end |
| `Accounts::connect`, `DbusTransport::open_with` and `call` (not the sheet methods), `DbusSession` | built; tested over a private bus against a fake inferd, the real session server and the real `AccountService` behind a bus adapter |
| `SocketTransport`, `InProcess::open_with` and their sessions, `DbusTransport` sheet calls | stub |
| D-Bus proxies and skeletons, introspection files in `dbus/` | frozen, introspection tested; skeleton methods answer `NotSupported` |
| D-Bus argument codec (needs, candidates, grants, tokens, refusal error names) | built, round-tripped over the wire signature |
| `accountd`, `syncd` | skeletons: build their service, print "not implemented", exit 2 |
| `inferd` | built: a daemon on the session bus (the checked-in `dist/` files install it) |
| protocol families, wire adapters, sign-in flows, discovery, persistence | not started (no provider or AI vendor code by decision) |

## 6. Recipes

**Add a capability kind or a field** (a vocabulary bump): a design/31 §2 row first; the struct in
`porter-core::capability`, its variant in `Capability` and `CapabilityKind`, the need in
`porter-core::need`, its arm in `matching::fit` with a table row per field, round-trip rows in
`porter-core/tests/round_trip.rs`; bump `VocabVersion::CURRENT`; the D-Bus codec's field names.

**Add a provider**: a file `providers/<id>.toml`; `tests/shipped_files.rs` in porter-provider
parses it. No code unless it needs a new family, issuer or discovery kind.

**Add a protocol family**: its `Family` variant; a crate or module implementing `Provider` and
`ProviderSession` (HTTP stays in that crate, outside the pure set); its variant in
`accountd`'s `FamilyProvider`; a conformance test against a recorded fake.

**Add an auth kind**: its `AuthKind` variant; its row in `secret_purpose`; the add-sheet flow in
accounts-ui.

**Add an accountd request**: the variant in `AccountsRequest` and its reply in `AccountsReply`;
its arm in `AccountService::handle`; the D-Bus member in `porter-dbus` (proxy and skeleton), then
regenerate and review `dbus/org.quire.Accounts1.xml`; a client method in `Accounts`.

**Add a dataset**: its `DatasetKind` variant and conflict rule; the dataset plug-in in syncd.

**Add a request kind** (a vocabulary bump when it changes a frozen type): its variant in
`InferRequest` and `RequestKind`, its arm in `kind()`, a row in `inferd::session::fits`, its
reply in `InferReply`, its events in `InferEvent` if it streams, the round-trip rows in
`porter-infer/tests/frames.rs`; a design/31 §5.5 line. Speech and computer use are the models:
they travel on the same `Open` fd and add no D-Bus member.

**Add an AI kind to the picker**: its variant in `AiKind` with its slug and the
`ai.model.<kind>.<tier>` rows in design/22; the `Need` it maps to in the comment on the enum.

**Add a data class**: its variant in `DataClass`, its floor in `Policy::proposed` (or a line in
`every_data_class_has_a_floor_decision` saying it goes anywhere), the `ai.floor.<class>` row.

**Add a wire adapter**: a `Model` implementation; its variant in `inferd`'s `AdapterModel`.

## 7. Test harness

`porter-fake` is the harness: `fake_service(ScriptedPrompter::answering([...]))` builds an
`AccountService` over the three fake providers (declared by `crates/porter-fake/providers/*.toml`),
`MemorySecrets` with their secrets filed, and `FixedClock(NOW)`. An app is
`Accounts::over(InProcess::new(service, app_id))`. `porter-client/tests/end_to_end.rs` is the
model. Tests never touch the real bus, the network, a keyring or the user's files. The bus tests
(`porter-client/tests/{dbus_open,dbus_served,dbus_accounts,connect}.rs`) start a private
`dbus-daemon` from a scratch config (`tests/common/bus.rs`: cleared environment, scratch HOME and
runtime directory, killed on drop; `dbus-daemon` must be on `PATH`) and serve a fake `Inference1`
(`tests/common/inferd.rs`), the real session server over scripted seams
(`tests/common/served.rs`) or the real `AccountService` behind a bus adapter
(`tests/common/accountd.rs`). The one place the environment names a bus (`Accounts::connect`) runs
in a child process the test starts with a private bus address.

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
| `AccountPlan` (configured) | `Account` + the provider's `ProviderSpec`; `Incoming`/`Outgoing`/`Tls` stay in mailo as the Mail family's endpoint detail |
| `AccountCaps` (discovered IMAP detail) | stays in mailo; its summary is a `Claim` of `Capability::Mail` at `Provenance::Discovered` |
| `AuthPlan::OAuth { issuer, scopes }` | `AuthKind::OAuthPkce` + `Issuer`; scopes follow from the granted kinds; client ids stay deployment config per issuer and channel |
| `AuthPlan::Password { username, sasl }` | `AuthKind::Password` or `AppPassword`; `Username`/`SaslMech` stay in mailo's IMAP family |
| `OAuthIssuer { Google, Microsoft }` | `porter_provider::Issuer` |
| `SecretKey { account, purpose }` | `porter_core::SecretKey` |
| `SecretPurpose::{IncomingPassword, OutgoingPassword, OAuthRefresh}` | the same variants |
| `SecretPurpose::AddressBook` | `SecretPurpose::ServicePassword(CapabilityKind::Contacts)` |
| `SecretPurpose::{OpenPgp, Smime}` (keyed by fingerprint) | not porter's: mail signing keys stay in mailo (FINDINGS) |
| `Credential::{Password, OAuth}` with `chrono` expiry | `porter_core::Credential::{Password, OAuth}` with `UnixSeconds`; the redacting `Debug` carries over |
| `Credential::{OpenPgp, SmimeKey}` | stay in mailo, as above |
| `mail-runtime` `Secrets` (sync, `get`/`put`/`forget`) | `porter_secrets::Secrets` (async, `get`/`put`/`delete`/`delete_account`); `MapSecrets` is `MemorySecrets`, `KeyringSecrets` becomes `Oo7Secrets` or a keyring store |
| `oauth.rs`, `signin.rs` (registry), `renewal.rs`, `loopback.rs` | behind `ProviderSession::access_token`/`renewed` in an OAuth family crate (not started); apps get `IssuedToken` (`Bearer`, `Xoauth2`) instead of credentials |
| `discover.rs` + `mail-proto` autoconfig | `Discovery::Autoconfig`/`WellKnown`/`JmapSession` in the Mail family's `Provider::discover` |
| `presets/` (provider table) | provider files in `providers/` |
| `latchkey` (agent lifecycle, socket/pipe) | `SocketTransport`'s carrier; frames are `porter_core::wire` |

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
  not, a bounded repair budget (a setting), and a check of the final text with `Shape::check`.
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
     steps are `cua_step`.
   - `AuditSink` is `audit::SessionAudit` over an `AuditOut` (a JSON-lines file in the daemon).
3. **Away.** The first event is `Waiting(readiness)` when the engine is not ready, then `Routed`,
   then the turn's events and one `Finished` per request, exactly as `session::step` decides.

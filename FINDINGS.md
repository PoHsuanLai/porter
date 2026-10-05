# Findings

Open items and standing facts. An entry names the condition that closes it.

## Stubs behind frozen interfaces

Each row names the lane of the accounts program (porter PLAN §6) that removes it.

| Where | Closes when |
| --- | --- |
| porter-secrets `Oo7Secrets` against a live Secret Service (no `todo!()`) | The integration scenario that runs accountd on a private dbus-daemon with a Secret Service (oo7-daemon or gnome-keyring); W2a tests cover oo7's file backend only, since no private-bus Secret Service was available in the jail |
| porter-secrets `KeyringSecrets` on real macOS and Windows stores | The `portable` workflow's first green run on macos-latest and windows-latest; locally only `cargo check --target x86_64-apple-darwin` and `x86_64-pc-windows-msvc` were run, and the 1280-unit Windows cut is exercised through keyring-core's mock |
| `.github/workflows/portable.yml` | The runner can load the workspace: members (inferd) name `../stoker` path dependencies, so the job needs stoker checked out beside porter (or the pinned git rev of quire CONSUMING.md section 1) before its first run |
| porter-service `AccountService::{add_account, reauthenticate}` (2, `add.rs`) | W3c (Nextcloud, generic): drive `Provider::sign_in` through `Sheets::conversation` and `sheet::step`, file the credentials, store the account, audit; the ignored `end_to_end.rs` test is rewritten to the flow |
| porter-service `AccountService::adopt` (1) | W3d: accountd's `[adopt]` table and a legacy-store seam (no PLAN row names it; E2 of the mailo extraction needs it) |
| porter-service `AccountService::relay_plan` (1) | W3g: read the credential, mint an access token for an OAuth account, build the `RelayAuth` |
| `ProcCallers` wiring (0 stubs) | accountd's main does not yet call `accountd::load_callers(system, user)` for `/etc/porter/callers.toml` (ready; inferd keeps its `[callers]` rows and `ProcPeers::new` reads `/proc`), and the sender's pid is read without the bus's `ProcessFD` (pid reuse): W3d (accountd binary) loads the files, and `ProcessFD` is a later hardening |
| porter-client `SocketTransport::open_authenticated` (1) | W3g: the reply frame with the relay's descriptor on `SCM_RIGHTS` |
| accountd `BusSheets` `consent`, `conversation`, `update`, `input` (4, `sheets.rs`) | W3d: the sheet host over `AccountsSheet1` (tested against a hand-written host) |
| porter-http `HyperHttp::send` (1) | W3c: hyper-util's client over tokio, hyper-rustls for https (the TLS stack joins `docs/workspace-deps.toml` use then: rustls with `ring` or `aws-lc-rs` changes `deny.toml`'s licences, so it is the first family's decision), timeout, size cap |
| porter-proxy `ImapRelay::step`, `SmtpRelay::step`, `HttpRelay::step`, `rewrite_head`, `relay` (5) | W3g; the rustls `Connect` needs `tokio-rustls` in the pinned block |
| porter-discover `HickoryDns` (feature `io`) against a real resolver | The integration scenario that runs discovery on a machine with DNS; W3a built it from mailo's `SystemDns` and tests the parsers and search over table fakes only |
| porter-discover `parse_ocs_capabilities` app keys | A recorded live Nextcloud capabilities answer (the owner's live smoke, PLAN fixtures): the `calendar`, `contacts`, `tasks` and `notes` keys and the rule "`dav` marks calendars and contacts present" are from the app's documented keys, and the fixtures are of that shape, not recordings |
| porter-discover autoconfig URLs carry no `?emailaddress=` | An interface ask: `EndpointUrl` refuses `?`, and `HttpRequest.url` is one, so mailo's address query cannot be sent. Most autoconfig hosts answer without it; the fix is a query-capable URL in `porter-http` (W3c or the owner) |
| porter-discover MX to ISPDB uses each parent of the MX host, not the public-suffix list | `psl` joins the pinned block (mailo uses `psl = "2"`); until then a miss costs a few 404s to the ISPDB, and nothing sensitive is sent |
| porter-oauth: no shipped `/usr/share/porter/clients.toml` yet, and the Microsoft client id in it | packaging, once the owner has registered the Entra app (D2); `ClientRegistry::from_paths` reads it. No Google row ships (Google is a TODO). Issuer endpoint URLs in `Issuer::endpoints` were written from the issuers' documented values and were not re-fetched in the jail (no network): re-check each against its current documentation when the Microsoft family (W5b) and OpenRouter (AI2) first run live |
| porter-oauth: mailo E4 | `from_mailo` + `clients_toml` read mailo's `oauth.json` and write the person's `clients.toml`; E4 calls them once and deletes mailo's `oauth.rs`, `signin.rs`, `renewal.rs`, `loopback.rs` |
| porter-oauth `Pkce::challenge`, `parse_redirect`, `exchange_code`, `refresh`, `revoke`, `LoopbackServer::{bind, wait}` (7) | W5a; `challenge` needs a SHA-256 in the pinned block (interface ask below) |
| porter-discover `autoconfig_urls`, `parse_autoconfig`, `well_known_urls`, `parse_jmap_session`, `parse_ocs_capabilities`, `probe_ports` (6) | W3a; `parse_autoconfig` needs an XML reader in the pinned block |
| porter-families `nextcloud`, `generic`: `discover`, `open`, `sign_in`, `revoke`, `access_token`, `renewed`, `SignIn::next` (7 each, from one macro) | W3c |
| porter-families `microsoft` (7) | W5b |
| porter-families `api_key`, `openrouter` (7 each) | AI2 |
| porter-infer `Broker::infer` (streaming into a `ChatSink`) | the first wire adapter (Ollama, then llama.cpp and vLLM through stoker's `model-openai-compat`) |
| inferd `speech::SpeechRunner::{transcribe, speak}` | the speech runner: needs stoker's `speech-host-client` and the speech host; the closed enums `SttBackend`, `TtsBackend` join then. A speech need is routed `Unavailable` and a speech request on a session is refused `Unsupported` until then |
| porter-client: the socket carrier needs the `socket` feature (without it nobody is reachable) and no agent serves it in this repo; `InProcess` has no broker unless the app hands one in | an agent hosts the core on the latchkey socket and reads `LinkHello` (see "Fill W4: porter") / porter's own `Broker` is built |
| the accountd binary and syncd serve their bus | accountd: W3d; syncd: W6a (inferd serves its own, see "Fill F3: inferd") |
| Google (`providers/google.toml` ships as a file, no family code, no `google` feature in `porter-families`, no gdrive replica) | **TODO, owner decision D1 (2026-10-05)**: W5c when the owner resumes it. Then: Calendar, People, Tasks, Drive AppFolder, Photos upload and picker, Gmail only with a BYO client, the 7-day reminder |
| `SettingsModule1` on accountd (`/org/quire/Accounts1/settings`) and inferd | accountd serves it in W3d and inferd in AI1a, from quire's `ds-settings` `live` module (W1b); no skeleton lives in porter-dbus |
| inferd `ai.structured.*` live rows (open_text, open_list, depth, repair_budget): the schema is a static `dist/inferd.settings.toml` (all four rows hands-off, `bounded`, page `intelligence`) and the values are read from `[ai.structured]` of `inferd.toml` at start; an out-of-range value falls back to its default and is logged. Open edge question: inferd has no `ds-settings` dependency because that would be a new porter -> quire edge the boundary table does not list; AI1a decides whether inferd takes it (`--write-schema` and `org.quire.SettingsModule1`) or keeps the static file | AI1a: ai.structured.* live rows |

## Fill F3: porter-client and the session server (2026-10-03)

Lane `f3-porter-client`. Built: the D-Bus codec, `Accounts::connect`, `DbusTransport::open_with`
and `DbusSession`, `DbusTransport::call` for the methods that answer at once, and in inferd the
fd session server (`inferd::serve`). `todo!()` bodies: 30 before, 21 after (the codec's 4,
`connect`, `DbusSession::{send, next}` and `DbusTransport::{call, open_with}` are gone; the
remaining ones are in the table above).

What the client does, and the choices made where the specs were silent:

- `Accounts::connect` tries each link in order. A D-Bus link is reachable when the session bus
  is (the daemons are found, and started by activation, at the first call, so inferd without
  accountd still connects: memoryd wants that). A `Socket` link is skipped: its carrier is not
  built, and porter-client may not reach a runtime without the `dbus` feature. No link is
  `TransportError::Unreachable`. The session bus is found through the environment by the bus
  library; the test runs the connecting half in a child process whose environment names a private
  bus.
- `Inference1.Open` returns a Unix socket; its frames are `porter_core::wire` envelopes.
  `DbusSession::next` is cancel safe (the partial frame stays in the session); `send` is not.
  `traceparent` rides in the `options` vardict under `OPTION_TRACEPARENT`.
- **Descriptors ride by order, not by position** (`ClientFrame::attachments`): a frame names `n`
  attachments when its highest `AttachIndex` is `n - 1` (a tool result's images count); the sender
  gives exactly `n` descriptors on one `sendmsg`, the receiver appends every descriptor it gets
  to one queue and the frame takes the first `n`. A frame that finds fewer, a stream that ends
  with unclaimed descriptors, or more than 32 queued ends the session. The kernel's delivery
  boundaries are therefore irrelevant. `InferSession::send_attached` (defaulted: refuses a
  non-empty list; Unix only) is the trait-level door; `DbusSession` and `FakeInferSession` fill it.
- A bus error is classified once (`porter_dbus::classify`): no bus or no owner for the name is
  `Unreachable`; the bus or daemon refusing the caller (`AccessDenied`) is `Denied` with the
  daemon's text (since "Fill W5: porter"); anything else is `Malformed` with the text.
- `Accounts::infer` reads before it reports a failed write: a daemon that refuses a session
  writes the refusal and hangs up, so the write can fail with the refusal waiting unread.
- The codec is one JSON-tree conversion (`json_value`, property-tested): a need is its kind slug
  plus its fields by name (`context` is `x`, a set is `av`, a nested enum is `a{sv}`); a
  candidate is its object path, label and a vardict of the rest including `account` (the path's
  segment is lossy, so the path is checked against the account, not parsed); a grant is
  `(id, rest)`; a token is `(kind, value, expiry)`. `null` has no D-Bus form: an absent key
  stands for it. Errors reuse `CoreError::MalformedFrame` with a "D-Bus argument" prefix.
- accountd's refusals are D-Bus errors named `org.quire.Accounts1.Error.<Slug in Pascal case>`
  (`refusal_error_name`, `refusal_from_error_name`, a total table); `DbusTransport::call` returns
  them as `AccountsReply::Refused`, as the in-process carrier does. accountd's skeleton must
  reply with exactly these names (a `zbus::DBusError` enum with this prefix produces them; the
  test adapter in `porter-client/tests/common/accountd.rs` is the model).
- The sheet methods (`Choose`, `AddAccount`, `Reauthenticate`) return a Request object whose
  `Response` signal carries the answer. The results vardict, the response codes and the
  subscribe-before-call race were left for the accountd lane; built in "Fill W4: porter".
- Vocabulary: a frame's `vocab` is not checked on receipt; an older client skips what
  it cannot parse (VocabVersion docs). A mismatch that parses is accepted.

The session server (`inferd::serve`): `serve_session(stream, spec, &Seams)` feeds the session
machine its inputs and carries out its effects over four seams (`Router`, `EngineHost`,
`TurnRunner`/`RunningTurn`, `AuditSink`). It owns no engine and starts none; the daemon's
`Inference1.Open` makes the socketpair, returns one end and spawns it. Tested over a socketpair
with scripted seams (`inferd/tests/serve.rs`) and from the client over a private bus
(`porter-client/tests/dbus_served.rs`).

### Interface asks closed here

- 1 (Route event): `SessionIn::Routed` carries a `RouteDecision { served, readiness }`; the
  phases hold `ServedBy` instead of `ModelRef`; `Waiting(readiness)` goes out at once when the
  engine is not ready (and on `SessionIn::EngineProgress`); `Routed` goes out once, with the
  first turn ("sent to <provider>" says where the person's data goes, so it waits for data). The
  ask's "`Phase::InTurn` needs the routed note" is therefore moot: a turn starts only after
  `Routed` went out, so `InTurn` implies it.
- 2 (audio): `SessionOut::Audio(AudioFrame)` for an accepted frame, and `SessionOut::EndAudio`
  for `EndOfAudio` (the ask named only the first; the engine needs both).
- 3 (cua begin): `CuaProgress { NotBegun, Begun }` on `Idle` and `InTurn`. A `CuaBegin` turn that
  ends in `InferReply::CuaStep(_)` (an empty `CuaStepReply` is the acknowledgement: there is no
  other reply variant for a begin) begins the run; a failed or cancelled one does not; a second
  begin starts another run. `CuaStep` before a begin is refused `Unsupported`, also when it comes
  up from the queue.
- 4 (queue): `InTurn.queued: Option<InferRequest>`. A request that fits queues (depth one); a
  second, or one that does not fit, is refused at once. The queued request starts when the turn
  ends however it ends, including after `Cancel`, which ends the running turn only; if the
  engine dies, the queued request is told too (a second `Finished(Failed(NotReady))`).
- 5 (receipt scope): `ConfirmReceipt.covers: Confidentiality`, the most open label the person
  agreed the content may take; `declassify` returns `Result<_, BeyondReceipt>` and refuses a
  `to` that is not at least as restrictive as `covers` (`Secret` for a confirmation that opens
  nothing). `Confidentiality` derives `Ord` (variant order; not the lattice order, which is
  `join`). Constructors of `ConfirmReceipt` gain the field (almanac `memfiles` and docket tests).
  Nothing in docket or almanac calls `declassify` yet.
- 23 (notes): `CuaStepRequest.notes: Vec<StepNote>`; `StepNote { from: NoteFrom::{Person,
  Agent}, text }`, oldest first, `Debug` shows lengths only. A note is input, never authority:
  cua-session fences an agent's note as untrusted.
- 61 (prompt class): `DataClass::Prompt` (slug `prompt`): what the person typed or said to the
  companion and what a reviewer or planner quotes of it. Its floor in the proposed AI policy is
  on this computer (`Policy::proposed`, setting key `ai.floor.prompt`, proposed beside
  `ai.floor.voice`). No `VocabVersion` bump (nothing persists grants yet; the amendment before
  this one did the same). Docket call site: `crates/action-review/src/infer.rs`,
  `REVIEW_CLASS` changes from `DataClass::Notes` to `DataClass::Prompt` (and its comment).
- 67: `Target::Centre` already exists in stoker's `cua-action`; porter has no executor, so the
  porter side is a wire row (`a_scroll_with_no_coordinate_and_a_bad_argument_cross_the_wire`).
  The executor is cua's: `seat-input::plan` must resolve `Target::Centre` against the window
  size it plans for. `DropReason::BadArgument` joined porter-infer's mirror (after
  `MissingArgument`, as in stoker's cua-parse).
- 70 (per-document class for embeddings): no porter-core change is needed. `EmbedRequest.class`
  and the session's class already pin one class per request and per session, and the machine
  refuses a request whose class differs. The shape to write on almanac's side: `recall::Doc`
  gains `class: DataClass`; `InferdEmbedder` partitions a batch by class, opens (and keeps) one
  session per class, and returns vectors in input order. A caution to record there: one index is
  one model, so a document's class cannot change *which* model embeds it, only whether that
  model may receive it (the class's floor against the model's locality); an index whose model
  is cloud would refuse its `Mail` documents. The embedder should therefore be pinned to a model
  that satisfies the strictest class in the Space (in practice on this computer).

### Call sites that should change now the client exists

- almanac `crates/memoryd/src/main.rs` (`run`, the line `let inferd = Arc::new(NoInference);`):
  build the session-bus connection first, then
  `Arc::new(AnyTransport::Dbus(DbusTransport::over(connection.clone())))` and give it to
  `InferdEmbedder::new` and `InferdConsolidator::new`; `memoryd/Cargo.toml` needs
  `porter-client` with `features = ["dbus"]`. If inferd is not running, `open` answers
  `Unreachable` and the daemon degrades as designed (lexical-only, no consolidation). Then
  `NoInference`/`NoSession` and the header comment can go. The embedder's `class` should follow
  ask 70 above.
- docket: `action-review/src/infer.rs` `REVIEW_CLASS` (ask 61); `ConfirmReceipt { .. }`
  literals (`docket-router/tests/{call_step,lifecycle,sessions,control}.rs`,
  `docket-fake/tests/fakes.rs`) gain `covers`; `intentd/src/system.rs`, `companiond` and
  `readerd` build `porter_client::AnyTransport` the same way memoryd does
  (`DbusTransport::over(connection)`); the router's reviewer pinning (ask 16) is now the
  `DataClass::Prompt` floor.
- cua: `cuad/src/model.rs` `InferdCuaModel::{begin, step}`: `Transport::open(Need::ComputerUse,
  DataClass::Screen, tier)`, `send(Request(CuaBegin))` and read to `Finished(CuaStep(_))` (the
  acknowledgement), then per step `send_attached(Request(CuaStep), vec![memfd])` with the
  `FrameAttachment` as a memfd and `FrameImage.source = Attached(AttachIndex(0))`; forward
  `ThoughtDelta` and `ActionProposed` to the sink. `cua-run/src/turn.rs` `turn` and
  `cua-fake/tests/fakes.rs` `blank_request` gain `notes` (the run's inbound messages become
  `StepNote`s). `seat-input::plan` resolves `Target::Centre`. Any exhaustive match on
  `DropReason` gains `BadArgument`.
- almanac `memfiles` settle(Keep) and tests construct `ConfirmReceipt` (ask 5, field `covers`).

## Fill F3: inferd (2026-10-03)

Lane `f3-inferd`. inferd is a daemon: `Inference1.Open` checks the caller, routes, makes the
socketpair and runs `serve_session` over four real seams; `Availability`, `Prepare`, `Usage`,
`Rescan`, `EnginesChanged` and `Gpu` are served too. `todo!()` bodies in inferd: 6 before (the two
`Engines` bodies, `local_claims`, `CuaRun::step`, the speech runner's two), 3 after (`CuaRun::step`
and the speech runner's two); 22 in the workspace before, 18 after (the other 15 are not this lane's). The tests are in `inferd/src/*/tests.rs`, `tests/serve.rs`,
`tests/hosted.rs` (the whole daemon on a private bus with fake engines) and `tests/dist.rs`.

Decisions, where the specs were silent:

- **Caller identity** is the executable behind the bus connection, through a caller table in
  `inferd.toml` (`[callers]`, the shape of memoryd's `callers.toml`). The role is `Cua` for cuad's
  executable and `App` for every other named one; only `Cua` is routed a computer-use need (`Denied`
  otherwise). A sender nobody names gets `AccessDenied` from the bus, which the client reports as
  `TransportError::Denied` with that text (`Malformed` before "Fill W5: porter").
- **Consent** for a model on this computer is granted (no grant to store: the data does not leave
  the machine, and the class's floor decides what may go where); any other account's model answers
  `Ask`, so a route to it is `NeedsGrant` until accountd can give grants. Spend is `Within` (nothing
  meters; local models are free).
- **Readiness** is `Downloadable` when the weights directory is not in the cache (looked for live,
  so a finished download shows at once); a `Downloadable` or `Unavailable` model is not offered,
  because nothing makes the weights. Stopped engines are `Loadable`: the first event of a session
  is `Waiting(Loadable)`, then `Routed`.
- **Floors**: the router needs no floor code of its own; `porter_infer::route` reads
  `Policy::floor(class)` (the proposed policy has `Prompt` on this computer), and a cloud-only
  route for a floored class is `RequiresCloud(class)` when the user's local-only switch is off, and
  `Unavailable` when it is on (the cloud is not offered at all).
- **Engines** are child processes of inferd (`ProcessHost`, unconfined and marked so); the unit in
  `dist/inferd.service` carries the sandbox for them (no network, read-only home, the GPU devices).
  Programs come from `inferd.toml`, never from the environment, and a kind with no program is not
  offered. The GPU probe reads only the total from `nvidia-smi` (`used_by_others` is zero: it
  counts our own engines, and subtracting them needs per-process accounting).
- **Retry** is stoker's `Retrying` around the chat provider (3 attempts, 250 ms doubling to 4 s);
  embeddings are not retried (memoryd degrades). Embedding texts are cut into batches of the
  model's limit and carry the model's prefix for the request's role.
- **Reasoning** left open (`EngineDefault`) is sent as `Off`: stoker's `Reasoning` has no unset, and
  a short interactive turn should not spend its tokens thinking. An app that wants it asks `On`.
- **`Prepare`** warms a stopped engine and answers `loading`; a later `Prepare` answers `ready`.
  `Rescan` only emits `EnginesChanged` (readiness is read live). `Usage` answers an empty
  dictionary.
- **Audit** goes to `$XDG_STATE_HOME/quire/inferd/audit.jsonl` as `AuditEntry` lines, not to
  memoryd: memoryd's `Record` has no area tag for inference and `prov` no system part for inferd,
  and porter does not depend on almanac. `images` and `audio_ms` are true since "Fill W4: porter":
  `AuditSink::record` is also given what the request carried.
- **Computer use** (the interim below was replaced in "Fill W5: porter" by stoker's `CuaSession`): `cua_step` runs one step for the tool dialects (`QwenComputerUse`, `Holo31`):
  `vision-prep::prepare` for a raw frame (an encoded one is sent as it is), a model turn with the
  `computer_use` function, `cua-parse`, `FrameMap` into window space, an out-of-frame point
  dropped. The prompt is ours and provisional, the history is the last four steps as text, and a
  reply that does not parse is not repaired. A model whose dialect is a text or wire one makes no
  computer-use claim. An empty `CuaStepReply` acknowledges `CuaBegin`.

Interface asks (nothing frozen was changed):

1. (closed in W4 with a `StepJob`) porter `inferd::cua_run::CuaRun::step(&mut self, request, sink)` cannot run: it has neither the
   pinned model's provider nor the bytes behind `ImageSource::Attached`. Proposed:
   `step(&mut self, job: StepJob<'_>, sink)` with `StepJob { model: &LocalModel, provider: &P,
   frames: &Frames, request: &CuaStepRequest }`, or delete it once `cua-session` is filled and
   `cua_step` shrinks to the model turn.
2. (closed in W5: `ModelEntry.embed`) stoker `ModelEntry` has no embedding fields: add `embed: Option<EmbedCaps>` (dims, batch limit,
   input limit, query and document prefixes, `EmbedCaps` already exists in `model-provider`), read
   when a role is `embeddings`. Until then `inferd.toml` carries `[[embedding]]` rows
   (`catalog::EmbedSpec`) and a catalog embedding entry without one makes no claim.
3. (decided in W5: the variant exists, inferd keeps `Off`) stoker `Reasoning` needs `EngineDefault` (or `Knob<Reasoning>` on `TurnRequest`) so an app that
   leaves reasoning open does not get it forced off.
4. (closed in W5: `structured`) stoker `model-extract::ExtractSession<T: Extract>` is typed; `ReplyShape::Json(String)` carries
   a JSON schema as text. inferd sends it to the engine as the constraint (`OutputShape::JsonSchema`)
   but does not validate and repair the reply (ARCHITECTURE section 7): a dynamic `Shape` built from
   schema text is needed.
5. (closed in W4: `Carried`) `AuditSink::record(&self, spec, served, reply)` should also be told what the request carried
   (images, audio milliseconds) so `AuditEntry.images` and `audio_ms` are true.
6. almanac and prov: an inference audit event needs `AreaTag::Inference` (almanac-core) and
   `SystemPart::Inferd` (prov) before a bridge from `audit.jsonl` to `Memory1.Record` can exist; and
   the porter to almanac edge it implies must be decided (almanac's repo graph is kept acyclic by
   design: "porter has no almanac dependency").
7. stoker `parse_entry` should refuse a llama-server profile whose weights are not GGUF
   (`command` already notes it).

Still needed before a real local model answers on login:

- Weights in the Hugging Face cache (downloaded by hand: there is no downloader), and
  `inferd.toml` with the engine programs (`vllm_python`, `llama_server`), the caller table and, for
  memoryd, a catalog entry for nomic-embed-text with its `embed` table (since W5; no `[[embedding]]` row).
- The engines' units: either a per-engine transient systemd unit (the sandbox of models section 3.9,
  `StartTransientUnit`; `ProcessHost` is the unconfined stand-in) or `dist/inferd.service` verified
  on the machine with the GPU (the `DeviceAllow` names are unverified, spike S1); llama-server's
  `--host <socket>` and vLLM's `--uds` need the same spike.
- The speech runner (`SpeechRunner::{transcribe, speak}`, stoker's `speech-host-client` and the
  host binary): a speech need is `Unavailable` today.
- stoker's `cua-session` bodies, and a recorded Holo-3.1-4B step to confirm the `computer_use`
  schema, the grid of 1000 and the reasoning-off sampling.
- Spend meters and caps behind `Usage`; per-process GPU accounting; telemetry spans from
  `traceparent`; validate-and-repair of structured output (ask 4).

### Call sites that should change now inferd serves

- almanac `crates/memoryd/src/main.rs`: already builds `DbusTransport::over(connection)`; its
  `callers.toml` is memoryd's own, and inferd's `[callers.apps]` must name `memoryd` too. The
  embedder's card (`default_card`: nomic-embed-text-v1.5, 768, `search_query: ` and
  `search_document: `) must equal the catalog entry's `embed` table (since W5).
- docket `intentd`, `companiond`, `readerd`: the same, and each executable in `[callers.apps]`.
  `action-review/src/infer.rs` `REVIEW_CLASS` is `DataClass::Prompt` (earlier note); the reviewer
  and the planner open `Need::Llm` sessions that this router answers with the model the tier map
  names.
- cua `cuad/src/model.rs` `InferdCuaModel::{begin, step}`: opens `Need::ComputerUse(Desktop)` with
  class `Screen` as the `cua` caller; the first request must be `CuaBegin`; steps carry the frame as
  attach index 0 (raw, `FrameLayout::Raw`, preferred); expect `Waiting(Loadable)` before `Routed`
  on a cold engine and `Finished(Failed(Unparseable))` for a reply with no call.
- sill / detent: `Inference1.Gpu` and `EnginesChanged` are live; `Prepare` on double-tap ⌘.
- quire `docs/workspace-deps.toml`: `tokio`'s `test-util` feature (a dev-dependency of inferd).

## Fill W4: porter (2026-10-03)

Lane `w4-porter`, branch `w4-porter` from f4c4d59. `todo!()` bodies in the workspace: 18 before, 8
after (the 10 gone: `SocketTransport::{call, open_with}`, `SocketSession::{send, next}`,
`InProcess::open_with`, `InProcessSession::{send, next}`, `CuaRun::step`, and the two of
`AccountService`, which refuse now). What remains is in the table at the top: `Oo7Secrets` (4),
`Broker::infer`, the speech runner (2), accountd's `SheetPrompter`.

**The Request-object flow** (accountd's `Choose`, `AddAccount`, `Reauthenticate`; design/31 §4.4):

- *Where.* `/org/quire/Accounts1/request/<sender>/<token>`: the caller's unique name with the colon
  dropped and each dot an underscore, then the `handle_token` the caller put in the call's
  `options` (`[A-Za-z0-9_]`, at most 64). A token that is not one, or is in use by a sheet still
  open, is `InvalidArgs`; with no token accountd mints `accountd_<n>`. The caller names the path
  so that it can listen before it calls. `porter_dbus::{request_path, request_namespace}`.
- *Code.* 0 the sheet finished with an answer, 1 the person closed it (`Refusal::Dismissed`), 2
  anything else, with `refusal` in the results (the `Refusal` slug: `denied` for "Don't Allow",
  `no_fitting_account`, `unavailable`, ...). A code the shape does not define, or results that
  break their code's promise, are refused by the reader (`TransportError::Malformed`), never
  guessed. `porter_dbus::{response_of, reply_of}`, round-tripped through the real `(u, a{sv})`.
- *Results.* `Choose`: the candidate's fields by name (`account`, `provider`, `subject`,
  `capability`, `restriction`, `grant`) plus `path` (`o`, checked against the account) and
  `label`. `AddAccount`: `account` (the exact id) and `path`. `Reauthenticate` and every code 1:
  empty.
- *The race.* A daemon may send `Response` before the caller has read the method's reply.
  `porter_dbus::Sheet::subscribe` puts a match rule on the Request interface under the caller's
  own namespace (and one on `NameOwnerChanged` for accountd's name) before the call; after it,
  `response(path)` reads what queued. A signal counts only from the connection that owns
  `org.quire.Accounts1` and from the returned path, so another process cannot answer for the
  person (tested with a forged `Dismissed` sent first). If the owner leaves the bus, the wait ends
  `TransportError::Closed`; it never hangs. A malformed `Response` is `Malformed`.
- *Daemon side* (`accountd` as a library: `serve`, `Manager`, `Grants`, `Tokens`, one `Account`
  object per account, `request`). The sheet runs in a task; the `Response` goes to the caller
  alone (unicast, verified: a second connection with a match rule hears nothing); the object is
  removed after it. `Close` (callers only: `AccessDenied` for another connection) aborts the call
  in flight, which takes the prompter's sheet down, and no `Response` follows; the caller leaving
  the bus does the same; a call that panics answers `unavailable`. A caller that dropped its
  future closes the sheet too (`porter_dbus::Closer`, spawned from `Drop` by the client).
- *Errors.* Refusals are `org.quire.Accounts1.Error.<Refusal>`; the bus's own errors keep their
  real names (`AccessDenied` for a caller accountd does not know, `InvalidArgs`,
  `UnknownObject`). The `DBusError` derive cannot do the second (it names every non-refusal
  `org.freedesktop.zbus.Error`), so `RefusedError` is written by hand.
- *Callers.* A `Callers` seam (`app_of(sender)`) says which app a bus sender is, as inferd's
  `Peers` does. `TableCallers` is the one implementation (hosts that know their clients, tests);
  the executable-behind-the-connection table belongs in `porter-dbus` for both daemons when the
  accountd binary serves. An app that holds no grant for an account cannot see its `Account`
  object: `Reauthenticate` on it is `UnknownObject` (no bulk enumeration).
- Not served: the `Account` properties (`Id`, `Provider`, `Label`, `State`, `Capabilities`), the
  manager's signals, `OpenAuthenticated`, the Settings module. `AddAccount` and `Reauthenticate`
  answer `unavailable` (no family signs in), which an app already handles.
- Client: `Accounts::reauthenticate(account, window)` (the request existed, the method did not).
  An empty `parent_window` is no parent, an empty `provider_hint` the provider list.

**`SocketTransport`/`SocketSession`** (feature `socket`; Unix sockets; a named pipe is not
built, and without the feature, or on Windows, `call` and `open_with` are `Unreachable`):
one connection per call, so a `Choose` that waits on a sheet blocks nothing (abandoning it is
closing the connection). An accountd call is one `AccountsRequest` frame and one `AccountsReply`
frame; a session is `porter_infer::LinkHello::Open(OpenFrame { need, class, tier, options })`,
then `ClientFrame`s and `InferEvent`s as on the bus's `Open` fd (descriptors ride as SCM_RIGHTS on
the frame that names them; a refusal is the first event). The agent reads the first frame once
and tells the two apart by its `kind` (`open` is not an `AccountsRequest` kind; tested). The
agent is not built here: the carrier is tested against a hand-written one (`tests/common/agent.rs`)
over the real `AccountService` and the real session server. The two session types share one
framed implementation (`framed`, feature `framed`, which `dbus` and `socket` both turn on; the
pure build still reaches no runtime). `Accounts::connect` reaches a socket link when the agent
accepts a connection.

**`InProcess::open_with`**: `InProcess<P, S, U, K, B = NoBroker>`; `with_broker(host)` hands in a
`SessionHost` (`open(app, need, class, tier, options)`, implemented for `Arc<T>` too). With no
broker a session is `Unreachable`, as when inferd is not running, so memoryd-style callers
degrade the same way in process. `InProcessSession` stays as `NoBroker`'s session, which cannot
exist. Porter's own `Broker` is still a stub, so nothing in this repo is a broker yet.

**`CuaRun::step`** (ask 97): `step(&mut self, StepJob { model, provider, frames, request }, sink)`
over `cua_step` (the run owns the history; a failed step is not remembered, so the same step can be
asked again; a sink `Stop` ends the proposals). `runner::Turns` runs computer-use steps through it.

**`AuditSink`** (ask 101): `record(spec, served, reply, &Carried)`; `Carried { images, audio_ms }`
is counted by the loop for the turn in flight (`serve::carried::Tally`): a chat's image parts
(tool results' too), one frame for a computer-use step, the milliseconds of accepted audio frames
at the turn's rate plus what a `Spoke` reply says it produced. A turn that failed or was
cancelled still reports what it carried. `AuditEntry.images`/`audio_ms` are true now.

`porter-fake`: `Scripted::Hang` (a sheet that stays open) and `AskLog::abandoned()` (how many asks
were dropped while they waited), for the close and leaving-caller tests.

### Interface asks from W4

1. **`TransportError::Denied` not added.** A bus refusal (`AccessDenied`) is still
   `TransportError::Malformed("refused by the bus: ...")`, because adding a variant breaks two
   exhaustive matches: almanac `crates/memoryd/src/infer/embed.rs:189` (`transport_failure`:
   `Unreachable | Closed`, `Malformed`) and cua `crates/cuad/src/model.rs:63` (`Unreachable`,
   `Closed | Malformed`). Proposed: `TransportError::Denied(String)` after those two matches gain
   an arm (almanac: unavailable or fatal; cua: `CuaModelError::Fatal`), then
   `DbusTransport`'s `bus_error` maps `BusFailure::Denied` to it. docket's `exit.rs` has no
   exhaustive match on porter's type (its `TransportError` is its own).
2. **The latchkey agent** (mailo's agent, or accountd built without zbus) reads the first frame of
   each connection as `AccountsRequest` or `LinkHello`, answers a call with one frame and
   serves a session as `inferd::serve_session` does after the hello; the peer is the connection's.
   design/31 §4.3 should say so (its "framed messages over latchkey's socket" does not name the
   session's first frame).
3. **design/31 §4.4** gains: `handle_token` in the sheet methods' `options` and the Request path
   form; code 2's `refusal` result and code 1 for `Dismissed`; the results keys above; `Close`
   by the caller only; an empty `parent_window` and `provider_hint`.
4. accountd binary: `ProcPeers`-like `Callers` (the executable behind the connection, a caller
   table in config), which wants `inferd::peers` moved to `porter-dbus` so both daemons share it;
   the `Account` properties and manager signals; the `SheetPrompter` over accounts-ui.
5. `Provider` needs a sign-in method (Login Flow v2 first) before `AddAccount` and
   `Reauthenticate` can leave `unavailable`.

### Call sites that change or may change after W4

- No exhaustive match downstream breaks: almanac (without recall-fastembed) and docket pass
  `cargo check --locked --workspace --all-targets --all-features` against this worktree; cua passes
  without `--locked` (its own `Cargo.lock` already fails `--locked` against porter master, with or
  without this lane). `InProcess` gains a defaulted fifth type parameter
  (`InProcess<P, S, U, K>` still names the no-broker form); nothing downstream names it yet.
- mailo's add flow (`Choose`/`AddAccount`) now has a real bus path: `Accounts::request_grant`,
  `add_account` and the new `reauthenticate` over `DbusTransport` work against accountd; against
  today's skeleton binary they are `Unreachable`. mailo standalone keeps `InProcess` (no broker:
  inference is `Unreachable`, which memoryd's degrade path already handles).
- sill/detent: an add-account or re-sign-in sheet is a Request whose `Response` arrives later;
  `AddAccount` answers `unavailable` until a family signs in.
- cua `cuad/src/model.rs`: `TransportError::Malformed` text for a refused caller starts
  "refused by the bus" (unchanged); see ask 1.
- almanac/docket/cua tests that implement `porter_client::Transport` need no change; a test that
  implements `inferd::serve::AuditSink` gains the `&Carried` parameter (none found outside this
  repo).

## The rig amendment (2026-10-03)

Interface changes made before any fill wave, from `research-rig.md` section 7 items 7 and 8.
Types, signatures, docs, pure tables and tests only; no new `todo!()` in porter (the count is
unchanged by it).

- `porter-infer`: `ChatControl` on `ChatRequest` (`tool_choice`, `tool_calls`, `max_output`,
  `reasoning`, `sampling`, `stop`), `ChatReply.{stop, thought}`, `TokenUsage.cached`,
  `ReplyShape::Choice`, `MessagePart::Thought(ThoughtPart)`, `EmbedRequest.role`,
  `OpenOptions`/`Traceparent`; `Transport::open_with` in porter-client.
- `porter-core`: `EmbedCap.{max_batch, prompts}` (`prompts` is boxed so `Capability` stays small);
  no `VocabVersion` bump, because no store persists capabilities yet.
- `porter-dbus`: an `options a{sv}` argument on `Availability`, `Open` and `Prepare`; the reserved
  key `traceparent`.
- `prov`: the `trace` module and `ActorKind::slug`, `Effect::slug`. These two are the one place a
  slug is written by hand, against the porter convention: a span attribute value must be a
  `&'static str`, which serde cannot give; a test pins each to its serde form.
- Decided, not left open: inferd owns retry and structured-output validate-and-repair
  (ARCHITECTURE.md section 7); `ContextOverflow` stays without a limit; `ChatControl.sampling` is a
  `Knob<Sampling>`, so an app that does not care writes `Off` and the model's catalog default
  applies (research-rig wrote a bare `Sampling`).
- For fill (kept as types, spelling left open): the engine-side constraint parameter names
  (llama-server's grammar and `response_format`, vLLM's structured-output parameter, Ollama's
  `format`) are stoker's to pin at fill; porter only carries `ReplyShape::{Json, Choice}`. The
  exact OpenTelemetry attribute names are verified against the current registry when the daemons'
  `telemetry.rs` is written; `prov::trace` holds only our own `quire.*` names.
- `inferd::bridge` maps `ChatControl` to stoker's `TurnRequest`, `StopReason` both ways,
  `ThoughtSeal`, `EmbedRole` and the prefix applied by inferd (built in fill F3, table-tested).
- almanac consumes `ChatRequest`, `EmbedRequest` and `EmbedCap`; its `recall` amendment (item 9)
  adds the role, the batch limit and the prompts on its side.

## Fill W5: porter (2026-10-03)

Lane `w5-porter`. inferd now reads what stoker's W4 gave it. No `todo!()` was left to fill in this
lane (the count of `todo!(` in the workspace stays 8, `inferd::speech` 2 of them; the speech runner
still waits for `speech-host-client`); the work was the call sites of stoker's asks
98, 99 (decided: kept), 100 and the `cua-session` wiring, and porter's half of item 113.

### Decisions

- **Ask 98, `ModelEntry.embed`.** `catalog::{claims_of, capabilities_of}` read `entry.embed`
  (`EmbedCaps`: dims, batch and input limits, the query and document prefixes) and
  `LocalModel::embed()` hands it to `bridge::embed_turns`. `EmbedSpec`, the `[[embedding]]` rows
  of `inferd.toml` and the `embeds` parameter of `local::build` are gone: the catalog is the one
  place a model's details are written, and a user who needs other prefixes writes a catalog file
  of the same id in `$XDG_DATA_HOME/stoker/catalog` (a user file replaces the system one). No
  override was kept, because nothing needs one. An old `[[embedding]]` table in an existing
  `inferd.toml` is ignored (the file has no `deny_unknown_fields`); the sample in `dist/` says
  where the details live now. An embedding-only entry has no chat fields, so the engine's cache
  is sized for its `max_input` (stoker's `vram.need` takes the context).
- **Ask 99, `Reasoning::EngineDefault`: not mapped, on purpose.** porter-infer has the variant
  ("whatever the engine does") and stoker's codec sends no reasoning field for it, but then the
  engine's chat template decides, and for the thinking models (Qwen3 and the like) that is to
  think. The user-facing rule "no reasoning unless asked" would break: a short interactive turn
  would spend its tokens thinking. So `bridge::request::reasoning` still sends `EngineDefault` as
  `Off`, `default_sampling` already treats a stoker `EngineDefault` as reasoning-off, and the
  tests that pin `Off` stay. Revisit when the catalog says per model whether the template thinks
  by default (a `reasoning_default` field would let `EngineDefault` pass through for the models
  that do not).
- **Ask 100, validate and repair (`structured`).** For `ReplyShape::Json(schema)` inferd calls
  `Shape::from_json_schema` (limits `open_text` 4096, `open_list` 256, `depth` 16, constants until
  the settings rows exist) and, when it reads, `choose`s the mode (the engine's constraint, else
  the synthetic `final_result` tool, else the prompted schema) and runs a `ShapedSession` with one
  repair around `provider.turn`. `ReplyShape::Choice` is `Shape::Choice`, and the app gets the bare
  string. The app sees a reply that passed `Shape::check` or `ModelError::Unparseable` (never
  repaired when cut by `MaxTokens`; a content filter is `Refused`). While the turns run only the
  thoughts stream: the text of a first attempt may be repaired, so the reply is told once, as one
  `TextDelta`, when it has passed. Usage is the sum of the attempts. Not checked, sent as before:
  a schema the vocabulary refuses (a number, a `pattern`, an open object) and any request that
  carries tools of its own (its turn may end in a call of one of them and rightly not in the
  shape).
- **`cua-session` wiring (`cua_step`, `cua_run`, `tee`).** `CuaRun` holds the `CuaBegin` and, from
  the first step on (the model is known there), a `CuaSession`. A step is `request`, one turn
  through a `TranscriptSink` (inside `tee::Tee`, which also streams the thoughts), `absorb_for`,
  and one more turn for a `StepOutcome::Repair`. `cua_step::open` builds the `CuaProfile` from the
  entry (`caps.computer_use` dialect, `caps.images.{rule, space}`, history = `per_prompt - 1`
  frames (3 for Holo means 2: `CuaProfile::for_model` with `FrameBudget::within(per_prompt)`), one repair, PNG) and the `TurnSettings` (the entry's `reasoning_off`
  sampling, `max_output`, reasoning `Off`, `ToolParallelism` from the batching, the flavor's
  extras). `PrevResult` is `StepResult` (`Done`, `Refused`, `NotRun`; `Failed`, `UserDeclined`
  and `UserActed` are `Refused` with their words; the call ids are minted, a tool or text dialect
  does not read them). The `DropReason` mirror is unchanged.
  Changes a reader of the old interim will notice: a reply that does not parse, or whose every
  action is refused, costs one more turn; a step whose reply never parsed counts as taken (stoker
  remembers it, the next prompt lists it) while one that failed to reach the engine does not; the
  past frames are in the prompt (the interim sent text only); the text dialect (UI-TARS) is
  served, so `catalog::cua_cap` claims for a `Text` dialect as well as a `Tool` one (a vendor
  `Wire` still makes no claim: it needs the vendor's backend).
- **Window contents and notes** are `ObservationIn::with_tree` and `with_notes` (stoker's W5 fields).
  stoker's `StepNote` is one line of text, so a note keeps who said it in its words ("The person
  says: ..."); the prompt shows it as `Note: ...`. (An interim text part did this for a day, until
  stoker's `w5-stoker` branch; this lane merges with it.)
- **Item 113, `TransportError::Denied(String)`.** Added with the daemon's text; `DbusTransport`'s
  `bus_error` maps the bus's `AccessDenied` (inferd's caller table, accountd's `Callers`) to it,
  where it was `Malformed("refused by the bus: ...")`. The two matches that would have broken are
  given an arm in follow-up branches, to merge together with this one: almanac
  `memoryd/src/infer/embed.rs` (`Denied` is `fatal(why)`, as `Malformed` was: asking again does not
  help) and cua `cuad/src/model.rs` (`Denied` is `CuaModelError::Fatal`).

### Tests

`structured` (shape table, tools-of-its-own, a fit, a repair that names the field and never echoes
the reply, an exhausted repair, a cut reply, a content filter, an engine failure, a bare choice, a
stopped listener), `tee`, `cua_step` over a scripted provider (the request carries the goal,
results, notes, tree and the frame; the settings come from the entry; a raw frame becomes a PNG; a
point outside the frame is dropped after one repair; a reply with no call is asked again once with
the same frame; a reply that never parses still counts; an engine failure does not; the prompt
never holds more frames than the model takes; a stopped sink asks nobody for a repair), `cua_run`,
`catalog` (the dialects that claim, the `embed` table), and the hosted harness (a JSON reply
repaired before the app sees it, a refused schema sent unchecked, a choice, a computer-use repair
and the earlier frame in the next step, `Denied` over a real bus). Nothing needs a GPU, a network
or the real bus.

### Interface asks from W5

1. **stoker `CuaSession::absorb_for`** could take the session by `&mut` or return the old one when
   a repair turn fails, so inferd need not clone the session (past frames included) before each
   step. `StepNote` could keep who said it (`NoteFrom`) instead of one line of text.
2. **A per-model `reasoning_default`** in the catalog (does the template think when nothing is
   said?) is what would let `Reasoning::EngineDefault` pass through safely (decision above).
3. **settings rows** for the structured-output limits and the repair budget
   (`ai.structured.open_text`, `open_list`, `depth`, `repairs`), constants in `structured.rs`
   today.

### Seams served

- inferd to stoker `model-catalog` (`embed`), `model-extract` (`ShapedSession`, `choose`) and
  `cua-session` (`CuaSession`, `TranscriptSink`); porter-client `TransportError::Denied` to
  memoryd's embedder (almanac) and cuad's model (cua).
- End to end scenarios (`~/rs-wt/integration/MAP.md`): memoryd embeds through inferd with the
  catalog's prefixes; the action reviewer and the policy writer get a checked JSON reply or
  `Unparseable`; cuad's step loop against a Holo engine, including a repair turn; an unknown caller
  gets `Denied`.

## The companion amendment (2026-10-03)

Interface changes made before any fill wave, from QUESTIONS "Persistent companion" and "One
message model" and `research-persistent-agent.md` sections C, D and E. Types, signatures, docs,
pure tables and tests; no new `todo!()` in porter (one pure function was built instead).

- `prov::Message`, the one message model (`message.rs`): `id`, `thread`, `in_reply_to`, `from`
  and `to` (`Address` = `AgentRef` + `SpaceId`), `kind` (`Note`, `Request`, `Report { status }`
  with `ReportStatus { Done, Failed, Cancelled, Progress }`), `parts` (`Part::{Text, Entity,
  Outcome, Undo}`), `label` (travels, the sender's taint included) and `sent`. There was no
  separate return or result type in `prov` or `porter-core` to remove; `porter-infer`'s
  `PrevResult` is the model transport's computer-use feedback, not an agent message, and stays.
  Docket and cua must not add a result type: a computer-use run's final result is a `Report`
  whose parts hold `Outcome` refs and text.
- The rule "a message carries no authority" is in `prov`'s crate docs, in `message.rs` and in
  `ARCHITECTURE.md` section 9 (repo rules).
- `AgentRef { Companion, Worker { task }, Cua { run }, User }` (the roster's party; `Reader`
  and `Planner` are both `Companion`), `Address`, `Crossing`, and `AgentRef::of(&Actor)`.
- `AgentRole::Worker { task: TaskId }`. `ActorKind` is unchanged: a worker is
  `ActorKind::Companion` (policy tables and retention classes key on the kind; a worker is the
  companion's agent). Pinned: `every_actor_has_a_kind` in `tests/shapes.rs` gained the worker
  row, the only golden that changed.
- New ids: `TaskId` (moved here from docket-core; SPEC section 2 names docket-core as its home,
  which must now read prov), `MessageId`, `ThreadId`, and the opaque `OutcomeRef` and
  `UndoHandle` (1 to 128 bytes, no control characters) that stand in for docket's `Handle` and
  `UndoId`, which prov cannot name.
- The desktop join rule (research D G15, user decision Q9), built and table-tested in
  `scope.rs`: `Confidentiality::join` (Secret absorbs; Public is the identity; Private sets
  union, and `desktop` is dropped whenever another Space is present, so it is a sub-scope of
  every Space); `Confidentiality::flow_to(&SpaceId) -> Flow` (every named Space must be the
  target or `desktop`); `desktop_admits(&Label) -> DesktopVerdict` (admits only `Trusted`,
  sourced from `{User}` alone, not `Secret`, not private to a real Space). `Label::join`
  stays a stub and is to call `Confidentiality::join` for its confidentiality half.
- A message's `Message::check` bounds (`MAX_PARTS` 64, `MAX_TEXT_BYTES` 16 KiB) are a
  proposal for the router to measure; they are constants, not settings.
- Open for docket: `Message.label` is one label for the whole message (the join of its
  parts). A typed ref part keeps its own label at its owner and is read through it.

## W1a: the accounts freeze amendment (2026-10-06)

Lane `w1a-accounts`, branch from 0571e0e. `todo!()` bodies: 8 before; 81 after, counting the five
family skeletons as the seven each they expand to (53 sites in source). The 74 new ones are the
table at the top. Design source: `design/31-ACCOUNTS.md` and `~/rs-wt/accounts/PLAN.md`.

Decisions, where the brief or the plan left a choice:

- **`Family` moved to porter-core** (`porter_core::Family`, with `slug`, `relay_protocol`,
  `serves`), because `ServiceEndpoint.family` names it and porter-core cannot depend on the
  provider crate. PLAN G1 wrote `FamilyRef`; there is no second type. `porter_provider::Family`
  is gone (no consumer named it).
- **No dependency list of porter-core, -provider, -secrets, -service, -client, -dbus, -infer,
  -fake or `prov` changed**, because almanac, docket, cua and sill lock them: a changed list is
  a `--locked` failure there. That is why `RelayPlan`, `ByteStream` and the in-memory `duplex`
  are in porter-core, why `StoreError` and `SheetFault` write `Display` by hand, and why the
  caller table's TOML is read by the daemon (`CallerTable` is serde only).
- **`OpenAuthenticated`**: `AccountsRequest::OpenAuthenticated { grant, endpoint: EndpointUrl }`
  and `AccountsReply::Authenticated` (no value: the descriptor is out of band). The endpoint is
  the url text a candidate listed, and accountd refuses one the account does not hold for the
  grant's kind (`Refusal::EndpointNotGranted`, `Registry::relay_target`). `handle` answers it
  `Unavailable`; `AccountService::open_authenticated` returns a `RelayPlan` and the transport
  makes the descriptor. `Transport::open_authenticated` is a provided method (default
  `Unreachable`) so consumers' test transports compile unchanged. `InProcess` takes a `RelayHost`
  (spawns the relay over `porter_core::stream::duplex`).
- **Sign-in types are split along the secret**: `porter_core::sheet::{Progress, SignInInput,
  SignInFault, FieldSpec, Review}` are secret-free values the machine and the views use;
  `porter_provider::{SignInStep, Signed}` carry the credential, and `SignInStep::progress` is the
  one mapping. `SignInStep::Review` carries the `Restriction` too, so the review step shows the
  limits. `SignInFault` has no issuer in `NeedsClientId`.
- **`Sheets` replaces `Prompter`** with `consent` and `conversation -> Link`; `SheetLink` is a
  trait (`update`, `input`) because the host owns the channel. `porter_core::sheet::Sheet::new`
  and `view` are built; `step` is the stub. `ScriptedPrompter` is `ScriptedSheets`.
- **Persistence**: `AccountService<P, S, U, K, R = NoStore, A = NoAudit>` saves after every
  change and records one `AuditEntry` per event; a failed save is `Unavailable` to the caller and
  the change stays in memory. Toggles are `Persisted.toggles` (`AccountToggle`); `Account`
  carries effective claims only. `VocabVersion(3)`; `FIRST_PERSISTED` is 3 and the migration
  table is empty, with a test that fails when a bump has no row.
- **The `IssueToken` audience check is filled**: an audience is a family slug (or the fixed
  endpoint a row names), and a grant covers the families that serve its kind in the provider
  file, plus `smtp` for a mail row that sends (`porter-service::audience`).
- **`Candidate.endpoints`** lists the account's endpoints whose family serves the matched kind;
  on the bus it is the vardict key `endpoints` (a list of records).
- **`porter-http` is a crate the brief did not list.** Every family, discovery step and OAuth
  exchange needs the one HTTP seam, and PLAN D12 allowed "a small `porter-http` client". Its
  hyper client is behind feature `hyper` (hyper, hyper-util, http-body-util, tokio, as stoker's
  `model-http` takes them); TLS (hyper-rustls) joins with the first family, see its row above.
- **`Manager.Adopt(legacy a{sv}) -> s`** is the D-Bus member of `AccountsRequest::Adopt`; accountd
  does not serve it yet. `LegacyRef` holds non-secret facts (the legacy account id, provider,
  label, endpoints, which of `Incoming`, `Outgoing`, `OAuth`, `AddressBook` exist); the legacy
  service comes from the daemon's `[adopt]` table by caller, never from the request.
- **`Sync1.Status` key `quota`** is `porter_dbus::STATUS_KEY_QUOTA`, a vardict with `used` and, when
  the provider reports a limit, `total` (both `t`); syncd converts porter-sync's `Quota`.
- `Replica::quota` is a required method; `MemoryReplica` counts live bytes and takes a limit
  (`with_limit`, refusing `PutRefused::Quota`).

### Interface asks from W1a

1. **quire `docs/workspace-deps.toml` needs**, before the lanes that use them: a SHA-256
   (`sha2`) for PKCE S256 (W5a); an XML reader (`quick-xml`) for autoconfig and WebDAV
   multistatus (W3a, W3b); `tokio-rustls` beside `hyper-rustls`/`rustls` and a decision between
   `ring` and `aws-lc-rs` (the latter brings `aws-lc-sys`, whose licence `deny.toml` does not
   allow) (W3c, W3g); `hickory-resolver` for the `Dns` seam (W3a); keyring-core and its
   platform stores (W2a).
2. **mailo, E4/E6/E7**: IMAP, SMTP and DAV engines take a pre-authenticated stream. The
   stream is `porter_client::AuthenticatedStream` (`Fd(OwnedFd)` on Unix, `Memory(DuplexEnd)`
   in process). An engine over a tokio socket wraps the fd with `UnixStream::from_std` and
   skips LOGIN/AUTHENTICATE and STARTTLS: it reads `* PREAUTH` (IMAP), `220` and an `EHLO`
   reply with no `AUTH`/`STARTTLS` (SMTP), and sends plain HTTP/1.1 (DAV). In process, mailo
   implements `RelayHost` by spawning `porter_proxy::relay` with a `Connect` (TCP plus its TLS
   choice) and wraps `DuplexEnd` as its engines' stream through `ByteStream`
   (`porter_proxy::TokioStream` is the other direction). Read the servers from
   `Candidate.endpoints` instead of `AccountPlan`'s own copies.
3. **mailo, E2**: `Accounts::adopt(LegacyRef)`; mailo must be named in accountd's `[adopt]`
   table (`org.quire.Mail = "mailo"`). In process it reads its own entries and files them
   through `Secrets` directly.
4. **sill (W3e)** serves `org.quire.AccountsSheet1` (`dbus/org.quire.AccountsSheet1.xml`): views
   are `porter_core::sheet::SheetView` as JSON, inputs `SheetInput` as JSON (a typed password
   rides in `FieldValue::Secret`). Only accountd's connection may `Open`.
5. **quire `ds-shell::accounts`** maps `SheetView` states: `Consent`, `Providers`, `SignIn`
   (fields are kinds; the UI words them), `BrowserWait`, `ShowCode`, `Review` (with the
   add-and-allow `allow` app), `Working`, `Failed`, `Done`.
6. **docket and cua `check-boundary.sh`** (PLAN §5): `Accounts::open_authenticated` and
   `adopt` join the forbidden calls in agent crates.

## Open

- inferd's stoker edges are by sibling path (`model-provider`, `model-catalog`, `engine-supervisor`, `model-http`, `model-openai-compat`, `vision-prep`, `cua-parse`, `speech-provider`), like `cua-action`; the pinned git revs replace them with quire's block. `cua-session`, `cua-vendors` (for `StepResult`) and `model-extract` are edges since W5; `speech-host-client` is not (its bodies are stubs).
- `InferSession` and its `SessionError` live in porter-infer (re-exported by porter-client), not in porter-client as models §3.8 words it: `porter-fake` implements the trait for `FakeInferSession` and must not depend on the client. `next()` returns `Result<InferEvent, SessionError>` (`Closed` after the daemon ends the session) where the spec says `InferEvent`.
- `DroppedAction`, `DropReason` and `SafetyHint` are named by the specs but not defined; porter-infer defines them (`SafetyHint` mirrors stoker's `SafetySignal`: `RequireConfirmation`, `Blocked`; both only add asks).
- The picker is a plain list (QUESTIONS V4): no recommended or best row and no ranking language; `Tier` stays because apps ask by tier and `Open` takes one. `picker_rows` breaks ties by the catalog's order, not by label.
- `inferd` has no `SettingsModule1` object yet (declared in design/22 §9.4; `INFERENCE_SETTINGS_PATH` names where it will live).
- `ai.floor.voice` (default on device) has no design/22 row yet; the quire agent adds `voice.*` and `ai.*` rows.

- Proxies are not run against skeletons (needs a zbus p2p test); closes with the codec.
- accountd has `FileStore` (`registry.json`) and `FileAudit` (`audit.jsonl`) in its library (W2b), but `main` does not use them yet: it must resolve `$XDG_STATE_HOME`, load the store (refusing a `StoreFault`) and pass both to the service. `SettingsModule1` belongs to design/22 §9.4. Closes with W3d (the daemon wiring).
- `PlanBudget` has no request budget; closes with ChatGPT sign-in (R9).
- Mail signing keys (OpenPGP, S/MIME) have no `SecretPurpose`; the user decides at the mailo migration.
- Proposed settings keys without design/22 rows: `ai.local_only`, `ai.floor.<class>` (voice and prompt included), `ai.spend.warn_permille` (800), `ai.model.<kind>.<tier>`.

## Fill F4: inferd on stoker 9052bf3 (2026-10-06)

- `cua_step::step` takes the run's `&mut CuaSession` and `CuaRun` keeps one session across steps (no clone, no `Failed.kept`); a step that fails after a repair calls `refill_repairs`.
- `Reasoning::EngineDefault` is sent as `EngineDefault` (it was `Off`); the sampling for it is `SamplingDefaults::for_reasoning`, which follows the entry's `reasoning_default`. Computer-use turns keep `Reasoning::Off` on purpose.

## Fill F4: inferd replay engine (2026-10-06)

Lane `f4-inferd-replay`. `[engines.<name>] replay = "<file>"` in `inferd.toml` names an engine that
plays a cassette; nothing else creates one, and the path is read from the table only. `inferd::replay`
serves the chat route (`POST /v1/chat/completions`, streaming SSE or one JSON body) and `/health` on
the engine's socket from inside the daemon; `ReplayHost` is an `EngineHost` that plays these engines
and hands every other to `ProcessHost`, so the router, the supervisor, the runner and the codec run
unchanged. The model is served under the table's name (a synthesised text, tools and structured
output entry with zero VRAM). The GPU is reported as it is: with only replay engines among the models the supervisor's headroom is zero (`Replays::supervisor_config`), so zero-VRAM engines fit on a computer with no `nvidia-smi`; any real engine keeps the default headroom.

- Cassette file: JSON Lines. Line 1 is stoker's `model-replay` `CassetteHeader` (`vocab` 1, `engine`,
  `model`, `recorded`, `context` whose `loaded` is the model's context, `speech` null). Every later
  line is an entry: `{"when": {"tools": "any"|"present"|"absent", "contains": [..], "lacks": [..]},
  "reply": <reply>, "uses": "once"|"always"}`. `when` reads the request's offered tools and the text of
  its messages; the first entry that admits a request and is not spent answers it (`once`, the default,
  is spent after one answer; the cursor lives for the daemon's life, not an engine process's). A reply
  is `{"kind":"text","v":"words"}`, `{"kind":"calls","v":[{"name":..,"arguments":{..}}]}` (the tool
  name as the planner offered it, `org.quire.Mail-mail.thread.search` for Mail), `{"kind":"fail","v":503}`
  or `{"kind":"wire","v":<model-replay WireReply>}` (a recorded exchange, frame by frame).
- Errors are typed and never reach the network: a missing or malformed file leaves the engine unable
  to start (`HostError::Refused`, logged at startup; the session ends `Failed`), and a request no entry
  answers gets HTTP 422 with `error.type = "replay_miss"` (`ReplayError::slug`).
- Why not match wire cassettes by request: `WireCassette` matches a request by its whole body; the
  planner's view carries handle numbers and prose the cassette's author cannot predict, so entries
  match on role (tools present, the writer's and the reader's fixed instruction) and play in order.
- `tests/cassettes/docket-flow-a.jsonl` is docket-accept flow (a) as the scripted `Inference1` plays
  it: the policy writer's draft (`uses: always`) and the planner's four steps (search, contact search, forward,
  the closing words). Flow (c) names handles from the planner's own view (`value #N`) and the docket
  side writes its cassette: planner steps in order, the reader as `{"when":{"tools":"absent",
  "contains":["fence"]},"reply":{"kind":"text","v":"{\"answer\":\"...\"}"}}`.
- Edges: inferd gains `model-replay` (workspace dependency, `check-boundary.sh`, ARCHITECTURE section 1).
  `EngineConfig` gains a flattened `named` map, so an unknown or misspelled scalar key under `[engines]` is now a config error instead of being ignored: a behaviour change for existing `inferd.toml` files (kept on purpose).

## Standing facts

- No ds-core: a closed set's serde form is its slug; UI crates map slugs to labels.
- `todo!()` is allowed only behind a frozen interface; every such stub is listed above.
- deny.toml is quire's verbatim (the unused MPL allowance warns).
- The provider file is `ProviderSpec`'s serde form; every field is written, none defaulted.
- porter-core's vocabulary is `VocabVersion(3)`: endpoints on `Account` and `Candidate`, `OpenAuthenticated`, `Adopt` and the refusal `EndpointNotGranted` joined (2 added computer use, `DataClass::Voice` and `GrantKey.space`). Version 3 is the first a file is written with (`FIRST_PERSISTED`), so a later bump needs a migration row.

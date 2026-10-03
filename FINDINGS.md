# Findings

Open items and standing facts. An entry names the condition that closes it.

## Stubs behind frozen interfaces

| Where | Closes when |
| --- | --- |
| porter-service `AddAccount`, `Reauthenticate` | the first family's sign-in (Nextcloud Login Flow v2) |
| porter-secrets `Oo7Secrets` | oo7 is in quire `docs/workspace-deps.toml` |
| porter-client `SocketTransport::{call, open_with}` and `SocketSession::{send, next}` (a socket link is skipped by `connect` until they exist), `InProcess::open_with` and `InProcessSession::{send, next}` | the latchkey carrier lands / an agent hosts the core |
| porter-client `DbusTransport::call` for `Choose`, `AddAccount` and `Reauthenticate` (they answer `TransportError::Malformed`, not a panic) | accountd serves Request objects (see "Fill F3") |
| porter-infer `Broker::infer` (streaming into a `ChatSink`) | the first wire adapter (Ollama, then llama.cpp and vLLM through stoker's `model-openai-compat`) |
| inferd `cua_run::CuaRun::step` | its signature cannot run (no engine, no frame bytes: see "Fill F3: inferd"); `cua_step` does the work for the tool dialects until stoker's `cua-session::{request, absorb}` exist, and `step` then takes the shape that file proposes |
| inferd `speech::SpeechRunner::{transcribe, speak}` | the speech runner: needs stoker's `speech-host-client` and the speech host; the closed enums `SttBackend`, `TtsBackend` join then. A speech need is routed `Unavailable` and a speech request on a session is refused `Unsupported` until then |
| accountd `SheetPrompter` | accounts-ui exists |
| accountd and syncd serve their bus | the items above (inferd serves its own, see "Fill F3: inferd") |

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
  `Unreachable`; the bus or daemon refusing the caller, or anything else, is `Malformed` with the
  text (there is no `Denied` variant on `TransportError`; adding one would break exhaustive
  matches).
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
  `Response` signal carries the answer. Not built: the results vardict, the response codes
  (portal's 0 ok, 1 cancelled, 2 other would map to nothing, `Dismissed`, `Denied`) and the race
  (a signal may be emitted before the client subscribes: subscribe by match rule on the
  `org.quire.Accounts1.Request` interface under a path namespace before the call, then filter by
  the returned path) are for the accountd lane to settle first.
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
  `TransportError::Malformed` with that text (there is no `Denied` variant on it, FINDINGS above).
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
  and porter does not depend on almanac. `images` and `audio_ms` are zero: `AuditSink::record` is
  told the reply, not the request.
- **Computer use**: `cua_step` runs one step for the tool dialects (`QwenComputerUse`, `Holo31`):
  `vision-prep::prepare` for a raw frame (an encoded one is sent as it is), a model turn with the
  `computer_use` function, `cua-parse`, `FrameMap` into window space, an out-of-frame point
  dropped. The prompt is ours and provisional, the history is the last four steps as text, and a
  reply that does not parse is not repaired. A model whose dialect is a text or wire one makes no
  computer-use claim. An empty `CuaStepReply` acknowledges `CuaBegin`.

Interface asks (nothing frozen was changed):

1. porter `inferd::cua_run::CuaRun::step(&mut self, request, sink)` cannot run: it has neither the
   pinned model's provider nor the bytes behind `ImageSource::Attached`. Proposed:
   `step(&mut self, job: StepJob<'_>, sink)` with `StepJob { model: &LocalModel, provider: &P,
   frames: &Frames, request: &CuaStepRequest }`, or delete it once `cua-session` is filled and
   `cua_step` shrinks to the model turn.
2. stoker `ModelEntry` has no embedding fields: add `embed: Option<EmbedCaps>` (dims, batch limit,
   input limit, query and document prefixes, `EmbedCaps` already exists in `model-provider`), read
   when a role is `embeddings`. Until then `inferd.toml` carries `[[embedding]]` rows
   (`catalog::EmbedSpec`) and a catalog embedding entry without one makes no claim.
3. stoker `Reasoning` needs `EngineDefault` (or `Knob<Reasoning>` on `TurnRequest`) so an app that
   leaves reasoning open does not get it forced off.
4. stoker `model-extract::ExtractSession<T: Extract>` is typed; `ReplyShape::Json(String)` carries
   a JSON schema as text. inferd sends it to the engine as the constraint (`OutputShape::JsonSchema`)
   but does not validate and repair the reply (ARCHITECTURE section 7): a dynamic `Shape` built from
   schema text is needed.
5. `AuditSink::record(&self, spec, served, reply)` should also be told what the request carried
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
  memoryd, the nomic-embed-text `[[embedding]]` row plus a catalog entry for it.
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
  `search_document: `) must equal the `[[embedding]]` row.
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

## Open

- inferd's stoker edges are by sibling path (`model-provider`, `model-catalog`, `engine-supervisor`, `model-http`, `model-openai-compat`, `vision-prep`, `cua-parse`, `speech-provider`), like `cua-action`; the pinned git revs replace them with quire's block. `cua-session`, `speech-host-client` and `cua-vendors` are not edges yet (their bodies are stubs).
- `InferSession` and its `SessionError` live in porter-infer (re-exported by porter-client), not in porter-client as models §3.8 words it: `porter-fake` implements the trait for `FakeInferSession` and must not depend on the client. `next()` returns `Result<InferEvent, SessionError>` (`Closed` after the daemon ends the session) where the spec says `InferEvent`.
- `DroppedAction`, `DropReason` and `SafetyHint` are named by the specs but not defined; porter-infer defines them (`SafetyHint` mirrors stoker's `SafetySignal`: `RequireConfirmation`, `Blocked`; both only add asks).
- The picker is a plain list (QUESTIONS V4): no recommended or best row and no ranking language; `Tier` stays because apps ask by tier and `Open` takes one. `picker_rows` breaks ties by the catalog's order, not by label.
- `inferd` has no `SettingsModule1` object yet (declared in design/22 §9.4; `INFERENCE_SETTINGS_PATH` names where it will live).
- `ai.floor.voice` (default on device) has no design/22 row yet; the quire agent adds `voice.*` and `ai.*` rows.

- Proxies are not run against skeletons (needs a zbus p2p test); closes with the codec.
- `IssueToken` does not check the audience against the grant (`Refusal::AudienceNotGranted` exists); closes with the first OAuth family.
- The registry is in memory (no persistence); `SettingsModule1` belongs to design/22 §9.4. Closes with accountd's store.
- `PlanBudget` has no request budget; closes with ChatGPT sign-in (R9).
- Mail signing keys (OpenPGP, S/MIME) have no `SecretPurpose`; the user decides at the mailo migration.
- Proposed settings keys without design/22 rows: `ai.local_only`, `ai.floor.<class>` (voice and prompt included), `ai.spend.warn_permille` (800), `ai.model.<kind>.<tier>`.

## Standing facts

- No ds-core: a closed set's serde form is its slug; UI crates map slugs to labels.
- `todo!()` is allowed only behind a frozen interface; every such stub is listed above.
- deny.toml is quire's verbatim (the unused MPL allowance warns).
- The provider file is `ProviderSpec`'s serde form; every field is written, none defaulted.
- porter-core's vocabulary is `VocabVersion(2)`: computer use joined; `DataClass::Voice` and `GrantKey.space` joined with it.
- A grant written before `GrantKey.space` existed does not parse: no store persists grants yet.

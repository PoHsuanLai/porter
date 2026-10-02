# Findings

Open items and standing facts. An entry names the condition that closes it.

## Stubs behind frozen interfaces

| Where | Closes when |
| --- | --- |
| porter-service `AddAccount`, `Reauthenticate` | the first family's sign-in (Nextcloud Login Flow v2) |
| porter-secrets `Oo7Secrets` | oo7 is in quire `docs/workspace-deps.toml` |
| porter-dbus codec (`need_to_dbus`, `need_from_dbus`, `candidate_to_dbus`, `candidate_from_dbus`) | accountd serves the bus |
| porter-client `connect`, `DbusTransport::{call, open_with}` and `DbusSession::{send, next}`, `SocketTransport::{call, open_with}` and `SocketSession::{send, next}`, `InProcess::open_with` and `InProcessSession::{send, next}` | accountd and inferd serve / an agent hosts the core |
| porter-infer `Broker::infer` (streaming into a `ChatSink`) | the first wire adapter (Ollama, then llama.cpp and vLLM through stoker's `model-openai-compat`) |
| porter-infer `picker_rows` | fill wave 1: the order and filter in its doc comment, a table test |
| prov `Label::{trusted_user, untrusted, join}`, `Labelled::zip`, `endorse`, `declassify` | fill wave 1: the FIDES lattice (integrity min, confidentiality through the built `Confidentiality::join`, classes and sources unioned), a proptest that `join` is a commutative, associative, idempotent semilattice (the confidentiality half already has an exhaustive test over a small universe) |
| inferd `session::step` | fill wave 1: the rows of models §4.2 and voice §3.4 as one table test (audio frames use `speech::check_audio`; a `Transcribe` turn on a class other than `Voice` or the caller's own is `Refused(Unsupported)`) |
| inferd `engines::{Engines::prepare, Engines::gpu}` | fill wave 3: the supervisor host over systemd transient units; needs stoker's `engine-supervisor` |
| inferd `catalog::local_claims` | fill wave 3: needs stoker's `model-catalog` (merge, then one `Claim` per model capability at `Provenance::Curated`) |
| inferd `cua_run::CuaRun::step` | fill wave 3: needs stoker's `cua-session` |
| inferd `speech::SpeechRunner::{transcribe, speak}` | fill wave 3: needs stoker's `speech-provider` and `speech-host-client`; the closed enums `SttBackend`, `TtsBackend` join then |
| accountd `SheetPrompter` | accounts-ui exists |
| daemons serve their bus | the items above |

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
- `inferd::bridge` still maps nothing: `ChatControl` to stoker's `TurnRequest` (sampling,
  `ToolChoice`, `ToolParallelism`, `Reasoning`), `StopReason` both ways, `ThoughtSeal` both ways,
  `EmbedRole` to `EmbedTurn.role`, and `EmbedCap.prompts` to the prefix applied by inferd, land
  with the pinned stoker revs of fill wave 1; each pair has a total mapping test then.
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

- inferd's stoker edges are not in the manifests yet. At the freeze porter path-patches only `cua-action`, because the other stoker crates (`model-provider`, `model-catalog`, `engine-supervisor`, `cua-session`, `speech-provider`, `speech-host-client`, ...) were still being frozen; `inferd::bridge` therefore holds only what needs none of them (`model_id_of`), and the `TurnRequest`/`TurnEvent` mapping lands with the pinned git revs of fill wave 1.
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
- Proposed settings keys without design/22 rows: `ai.local_only`, `ai.floor.<class>` (voice included), `ai.spend.warn_permille` (800), `ai.model.<kind>.<tier>`.

## Standing facts

- No ds-core: a closed set's serde form is its slug; UI crates map slugs to labels.
- `todo!()` is allowed only behind a frozen interface; every such stub is listed above.
- deny.toml is quire's verbatim (the unused MPL allowance warns).
- The provider file is `ProviderSpec`'s serde form; every field is written, none defaulted.
- porter-core's vocabulary is `VocabVersion(2)`: computer use joined; `DataClass::Voice` and `GrantKey.space` joined with it.
- A grant written before `GrantKey.space` existed does not parse: no store persists grants yet.

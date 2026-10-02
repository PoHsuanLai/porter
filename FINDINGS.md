# Findings

Open items and standing facts. An entry names the condition that closes it.

## Stubs behind frozen interfaces

| Where | Closes when |
| --- | --- |
| porter-service `AddAccount`, `Reauthenticate` | the first family's sign-in (Nextcloud Login Flow v2) |
| porter-secrets `Oo7Secrets` | oo7 is in quire `docs/workspace-deps.toml` |
| porter-dbus codec (`need_to_dbus`, `need_from_dbus`, `candidate_to_dbus`, `candidate_from_dbus`) | accountd serves the bus |
| porter-client `connect`, `DbusTransport::{call, open}` and `DbusSession::{send, next}`, `SocketTransport::{call, open}` and `SocketSession::{send, next}`, `InProcess::open` and `InProcessSession::{send, next}` | accountd and inferd serve / an agent hosts the core |
| porter-infer `Broker::infer` (streaming into a `ChatSink`) | the first wire adapter (Ollama, then llama.cpp and vLLM through stoker's `model-openai-compat`) |
| porter-infer `picker_rows` | fill wave 1: the order and filter in its doc comment, a table test |
| prov `Label::{trusted_user, untrusted, join}`, `Labelled::zip`, `endorse`, `declassify` | fill wave 1: the FIDES lattice (integrity min, confidentiality max with Private sets unioned, classes and sources unioned), a proptest that `join` is a commutative, associative, idempotent semilattice |
| inferd `session::step` | fill wave 1: the rows of models §4.2 and voice §3.4 as one table test (audio frames use `speech::check_audio`; a `Transcribe` turn on a class other than `Voice` or the caller's own is `Refused(Unsupported)`) |
| inferd `engines::{Engines::prepare, Engines::gpu}` | fill wave 3: the supervisor host over systemd transient units; needs stoker's `engine-supervisor` |
| inferd `catalog::local_claims` | fill wave 3: needs stoker's `model-catalog` (merge, then one `Claim` per model capability at `Provenance::Curated`) |
| inferd `cua_run::CuaRun::step` | fill wave 3: needs stoker's `cua-session` |
| inferd `speech::SpeechRunner::{transcribe, speak}` | fill wave 3: needs stoker's `speech-provider` and `speech-host-client`; the closed enums `SttBackend`, `TtsBackend` join then |
| accountd `SheetPrompter` | accounts-ui exists |
| daemons serve their bus | the items above |

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

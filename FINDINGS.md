# Findings

Open items and standing facts. An entry names the condition that closes it.

## Stubs behind frozen interfaces

| Where | Closes when |
| --- | --- |
| porter-service `AddAccount`, `Reauthenticate` | the first family's sign-in (Nextcloud Login Flow v2) |
| porter-secrets `Oo7Secrets` | oo7 is in quire `docs/workspace-deps.toml` |
| porter-dbus codec (`need_to_dbus`, `need_from_dbus`, `candidate_to_dbus`, `candidate_from_dbus`) | accountd serves the bus |
| porter-client `connect`, `DbusTransport`, `SocketTransport`, `InProcess::infer` | accountd serves / an agent hosts the core |
| porter-infer `Broker::infer` | the first wire adapter (Ollama) |
| accountd `SheetPrompter` | accounts-ui exists |
| daemons serve their bus | the items above |

## Open

- Proxies are not run against skeletons (needs a zbus p2p test); closes with the codec.
- `IssueToken` does not check the audience against the grant (`Refusal::AudienceNotGranted` exists); closes with the first OAuth family.
- The registry is in memory (no persistence); `SettingsModule1` belongs to design/22 §9.4. Closes with accountd's store.
- `PlanBudget` has no request budget; closes with ChatGPT sign-in (R9).
- Inference is one-shot; no stream chunk type. Closes with the first streaming adapter.
- Mail signing keys (OpenPGP, S/MIME) have no `SecretPurpose`; the user decides at the mailo migration.
- Proposed settings keys without design/22 rows: `ai.local_only`, `ai.floor.<class>`, `ai.spend.warn_permille` (800).

## Standing facts

- No ds-core: a closed set's serde form is its slug; UI crates map slugs to labels.
- `todo!()` is allowed only behind a frozen interface; every such stub is listed above.
- deny.toml is quire's verbatim (the unused MPL allowance warns).
- The provider file is `ProviderSpec`'s serde form; every field is written, none defaulted.

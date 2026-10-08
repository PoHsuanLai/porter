# Findings

Open items and standing facts. An entry names the condition that closes it.

## Stubs behind frozen interfaces

Each row names the lane of the accounts program (porter PLAN §6) that removes it.

| Where | Closes when |
| --- | --- |
| porter-secrets `Oo7Secrets` against a live Secret Service (no `todo!()`) | The integration scenario that runs accountd on a private dbus-daemon with a Secret Service (oo7-daemon or gnome-keyring); W2a tests cover oo7's file backend only, since no private-bus Secret Service was available in the jail |
| porter-secrets `KeyringSecrets` on real macOS and Windows stores | The `portable` workflow's first green run on macos-latest and windows-latest; locally only `cargo check --target x86_64-apple-darwin` and `x86_64-pc-windows-msvc` were run, and the 1280-unit Windows cut is exercised through keyring-core's mock |
| `Secrets::put_if_absent` is atomic only on `MemorySecrets` and the oo7 file backend (within one process, under the keyring lock); the Secret Service (`Keyring::DBus`) and keyring-core stores read then write, so another process can file the key in the window. The trait's default body is that read-then-write, so existing implementors (mailo, accountd tests) compile unchanged | the owner's live run against a real Secret Service/keychain; mailo's `adopt` switches to it later and treats `Stored` as the win |
| `.github/workflows/portable.yml` | The runner can load the workspace: members (inferd) now take stoker and quire by git rev (I5), so the job needs network (and credentials for the private stoker repo) but no sibling checkout |
| inferd probes `127.0.0.1` only, at the ports of `[probe]`: Ollama `11434`, llama.cpp `8080`, LM Studio `1234` | design/31 §3.3's "URLs the user listed" (vLLM, a LAN peer, ComfyUI on `:8188`) and "when a user unit named in settings starts": a settings row for extra URLs; `porter_discover::probe_ports` is loopback-only today, and a LAN host is never auto-probed by design |
| a stopped runtime is seen at the next look (at most `[probe] longest_s`, 120 s by default; `Rescan` looks at once), and a turn that finds it gone does not ask for a look | a turn that fails `Unreachable` on a probed model asking `Probing::now`; until then the person's next turn is refused `Unavailable` once the look has seen it |
| a runtime account the person removes in Settings comes back the next time inferd reports that runtime (a start of the runtime, a changed model list) | a setting that ignores a runtime (`ai.runtime.<provider>.ignore`), written by detent and read by inferd's probe; not asked for yet |
| probed models: licence is `open("unknown")` (a runtime does not say), no `StructuredOutput` feature (a probe cannot know it), the context is what the runtime reports or the 2048 floor, and a model with no list name that makes its id is left out | a probe that reads `/api/show` licence and a one-call structured-output check (`Probed` claims) |
| the Alert before a removal | detent confirms (`Alert{Critical}`, its `ActionWeight::Destructive` row) before it sets `accounts.<id>.remove`; porter-core has no `SheetView` for it, so accountd draws none. If the owner wants accountd to ask too, it needs a `SheetView::ConfirmRemove` (porter-core, an interface ask) |
| accountd settings: a service toggled back on | reads as its provider file declares it until the next discovery refreshes it: `Persisted` keeps only effective claims, so the pre-toggle `Discovered` parameters are not kept |
| accountd `Account` object per account is registered at start and on `AccountAdded`; an account the registry gains outside accountd's own calls (a second accountd process) is not seen | no second writer exists; the registry store is single-writer |
| porter-discover `HickoryDns` (feature `io`) against a real resolver | The integration scenario that runs discovery on a machine with DNS; W3a built it from mailo's `SystemDns` and tests the parsers and search over table fakes only |
| porter-discover `parse_ocs_capabilities` app keys | A recorded live Nextcloud capabilities answer (the owner's live smoke, PLAN fixtures): the `calendar`, `contacts`, `tasks` and `notes` keys and the rule "`dav` marks calendars and contacts present" are from the app's documented keys, and the fixtures are of that shape, not recordings |
| porter-discover MX to ISPDB uses each parent of the MX host, not the public-suffix list | `psl` joins the pinned block (mailo uses `psl = "2"`); until then a miss costs a few 404s to the ISPDB, and nothing sensitive is sent |
| porter-discover reports a document's OAuth2 offer (`Found.oauth`) and POP3 servers (`Found.pop3`, from a document or from `_pop3s._tcp` SRV, RFC 6186) as data, under `SearchOptions` | porter's endpoints stay IMAP and SMTP: the issuer-to-provider mapping (`issuer_named`, `issuer_for_server`), the choice of OAuth over a password, and the POP3 account are the caller's (mailo's E3 follow-up). `OAuthOnly::Offer` makes an OAuth2-only server an endpoint, which is a finding the caller must sign in to with OAuth2 or drop |
| porter-oauth: no shipped `/usr/share/porter/clients.toml` yet, and the Microsoft client id in it | packaging, once the owner has registered the Entra app (D2); `ClientRegistry::from_paths` reads it. No Google row ships (the owner registers one, docs/google.md). Issuer endpoint URLs in `Issuer::endpoints` were written from the issuers' documented values and were not re-fetched in the jail (no network): re-check each against its current documentation when the Microsoft family (W5b) and OpenRouter (AI2) first run live |
| porter-oauth: mailo E4 | `from_mailo` + `clients_toml` read mailo's `oauth.json` and write the person's `clients.toml`; E4 calls them once and deletes mailo's `oauth.rs`, `signin.rs`, `renewal.rs`, `loopback.rs` |
| porter-oauth: `LoopbackServer::wait` ends on a wrong `state` (a deliberate change from mailo, which kept listening) | `wait_until(expected, deadline)` restores mailo's behaviour for a host that asks: a stray request (wrong `state`, probe, malformed, oversized) gets the fixed error page and the wait goes on, bounded by `MAX_STRAY_REQUESTS` (8), the deadline, and `STRAY_READ_SECONDS` (5) per connection; E4's mailo opts in |
| porter-oauth: a token answer without `expires_in` (RFC 6749 5.1 RECOMMENDED, not required) | `TokenResponse.expires_in` defaults to `DEFAULT_EXPIRES_IN_SECONDS` (3600, mailo's assumption) in the parse, so the answer reads instead of being `Unreadable` |
| porter-oauth: the issuer's `error_description` for a host to show (mailo E4 ask 3) | Not a change to `ExchangeFault::Refused` (it is `Copy`, matched in porter-families, porter-oauth's renewal and device, and mailo's E4 patch): `exchange_code_scoped_detailed` and `refresh_scoped_detailed` return `ExchangeFailure { fault, says: Option<IssuerSays { error, description }> }`, text control-stripped and capped at `SAYS_MAX_CHARS` (200); the plain calls are unchanged. `revoke`, `mint_key` and the device poll keep the fault alone |
| porter-oauth: the redirect URI's path (mailo registered `http://127.0.0.1:<port>`, porter's default is `.../`) | `LoopbackServer::bind_with(RedirectPath::{Slash, Bare})`; `bind()` is `Slash`, unchanged. `redirect_uri()` must be the string sent to both the authorize and the token call. The listener accepts a redirect under either form |
| A unit row (`inferd.service`, `sill-shell.scope`) names a unit of the person's systemd user manager, which any process of theirs can start under that name while the real one is not running (`systemd-run --user --unit=…`); a system unit (`/system.slice`) cannot be squatted that way. Scopes match only exactly and only outside the `app-` namespace | hardening: read the unit's `ExecStart`/main pid through the manager (or `ProcessFD` + pidfd) before granting a unit row's role; until then a unit role is as strong as "the unit was running first" |
| porter-proxy `relay` runs the two directions in one loop (read either side, then write the other), because `ByteStream` has no split halves: a peer that stops reading while the other side sends more than the kernel buffers can stall it. IMAP, SMTP, ManageSieve and HTTP/1.1 alternate request and response, so this needs a pipelined upload against a streaming download at once | a split-halves addition to `porter_core::stream` if a workload shows it; no lane needs it now |
| porter-proxy standing limits: the HTTP relay refuses `Upgrade`, `CONNECT`, HTTP/2 and any head or chunked body it cannot frame exactly (so a request is never read two ways); the SMTP relay answers a repeated `EHLO` from the stored reply without an `RSET`; the ManageSieve relay is a byte relay after login, so an app's own `CAPABILITY` sees the server's `SASL` and `STARTTLS` lines | standing |
| porter-families `openrouter` (7): the sign-in that mints a key through the browser (OAuth PKCE, `porter_oauth::mint_key`), as a second sign-in on `providers/openrouter.toml`, which signs in with a pasted key today (the `api_key` family serves it) | the owner asks for it |
| accountd keeps nothing for spend and no model list | inferd meters (`cloud::spend`, AI2b); the curated catalogue (stoker) is the model list |
| `Peer.ResolveKey` does not spend a `GrantScope::Once` grant | an interface ask: whether a once-grant is spent by its first key (the audit side, `KeyResolved`, is done: lane p5-audit) |
| `accountd add --allow` records one grant per data class (all twelve unless `--class` narrows it), interactive use only, because a grant is exact on (app, account, kind, class, usage) | a background grant, or a class-wide grant, needs the Settings pane or a wider `GrantKey`; `ai.floor.<class>` still decides which classes may leave this computer |
| `accountd add` runs only with the daemon stopped (it holds the bus name as the lock); the registry has one writer | `AccountService` taking a second writer's changes (an interface ask for a `merge`/reload method in porter-service), or the sheet host (sill W3e) making the command unnecessary |
| `accountd add` against the real Secret Service and the real company endpoints: the library path is tested with in-memory secrets and loopback fakes, the binary only through its refusals and `--help` | the owner's live run: `accountd add openrouter --allow org.quire.Companion` |
| the key checks' endpoints and headers (`GET /v1/models` with `x-api-key` and `anthropic-version: 2023-06-01`, `x-goog-api-key` on `/v1beta/models`, bearer on `/v1/models`, OpenRouter's bearer `GET /api/v1/key`) and each keys page are from the companies' documentation as remembered, not fetched (no network) | the owner's live run; re-check each against the company's current documentation |
| porter-families `microsoft`: `MicrosoftProvider::new(spec)` reads `/usr/share/porter/clients.toml` and `$XDG_CONFIG_HOME/porter/clients.toml` and needs `HyperHttp::send` (W3c) and `/dev/urandom` (no Windows randomness: `getrandom` is not in the porter dependency block) | W3d builds it with `MicrosoftEnv`; the Windows path needs `getrandom` in the pinned block (interface ask) when mailo hosts the family there |
| porter-families `microsoft`: probe paths, scope names and the personal-domain list are from Microsoft's documentation and mailo, not a recorded live answer; personal vs work is by address domain (a personal account on a custom domain is classed Work) | The owner's live smoke once the Entra app exists (D2) |
| porter-infer `Broker::infer` (streaming into a `ChatSink`) | the first wire adapter (Ollama, then llama.cpp and vLLM through stoker's `model-openai-compat`) |
| inferd `speech::SpeechRunner::speak` (one `todo!()`) | the text-to-speech runner: `TtsBackend` over stoker's `TextToSpeech` (Kokoro-FastAPI as a supervised engine, `OpenAiSpeech`); a `Speak` request on a session is refused `Unsupported` and a speech need that asks for `Tts` is routed `Unavailable` until then. Speech to text is built ("Fill voice-inferd" below) |
| porter-infer `AiKind` (deprecated alias of `Slot`, with its variant-named constants and the old slugs `llm`, `speech_in`, `speech_out` still read) | removed once docket, almanac and cua name `Slot` (the slots lane's consumers); then the old `ai.model.<kind>.<tier>` rows stop loading one release after detent writes the slot rows |
| inferd pipeline stages `Describe` (images through the `image_in` slot) and `Speak` (the answer through `voice_out`) answer `Refusal::NotYet`, so a plan that needs them is `Unsupported`. A `Hear` stage runs on a live session (a voice chat, see the Voice section) | the pipeline lane that runs them (Describe needs a vision model in the catalogue; Speak needs the text-to-speech runner above) and |
| porter-client: the socket carrier needs the `socket` feature (without it nobody is reachable) and no agent serves it in this repo; `InProcess` has no broker unless the app hands one in | an agent hosts the core on the latchkey socket and reads `LinkHello` (see "Fill W4: porter") / porter's own `Broker` is built |
| porter-client `socket` takes the agent's address, knock and start from latchkey (`SocketAgent`): `Agent::connect` / `connect_or_start` -> `latchkey::into_fd` -> `std::os::unix::net::UnixStream::from` -> `set_nonblocking(true)` -> `tokio::net::UnixStream::from_std`; SCM_RIGHTS rides on that plain stream. The Windows file keeps tokio's pipe client (`into_fd` is Unix only) | **latchkey ask: done** (latchkey 335d7a7 `into_fd`, taken by final-batch); row stays only for the Windows pipe, which has no descriptors |
| porter-client `engines` (`EngineHost`) serves `Need::Llm` chat and task only: embeddings, speech, computer use, spend caps, the audit trail, the picker and a probe for `prepare` (it answers from routing, `Ready`) are inferd's and not here; no `https` test (the fakes are plain loopback; the TLS path is stoker's `HttpClient` as inferd's hosted models use it); `engines` is not checked for Windows (ring's C build needs the MSVC toolchain), only `porter-bridge` and the `socket` pipe are | An app that needs them asks; embeddings are the nearest (`porter_bridge::embed_turns_for` is there) |
| quire's `docs/workspace-deps.toml` still pins `latchkey` as the old copy inside mailo (`PoHsuanLai/mailo` rev 43ebf45); porter names the standalone repo at 7b250b9, the rev mailo pins now (`[workspace.dependencies]`, one line) | quire's pinned block moves latchkey to `PoHsuanLai/latchkey` at 7b250b9 (or later); porter then copies that line |
| porter-client `StartAgent::Spawn` starts the agent as this same binary (`latchkey::spawn`); an app that starts it another way (a separate executable, launchd, a service) has no policy for it yet, so it starts the agent itself before `Accounts::connect` | an app that needs it: a `StartAgent` variant carrying the app's own closure (it would cost `PartialEq` on `LinkChoice`) |
| Google: the family is built (lane google-route, 2026-10-08; section at the end of this file) and waits for the owner's own Google Cloud client | the owner registers the client and writes its row (docs/google.md). Still to come, its own lane: the Google calendar source for syncd's PIM mirror (after graph-calendar's source seam). The Drive app-folder replica (`storage-gdrive`), Google Photos upload and the picker import landed with lane google-drive (section at the end) |
| inferd `ai.structured.*` rows are read at start only: `Inference.limits` is fixed when the daemon builds it, so a page edit of them needs a restart (the other `ai.*` rows reload; `Limits` would join `settings::Settings`). The rows themselves are in the schema, read at `[ai.structured]`, | a later inferd lane |
| provider files: an SMTP endpoint is a second `kind = "mail"` row with `family = "smtp"` and `transport = "imap"` (`MailTransport` has no SMTP: "IMAP, with SMTP for sending"); `microsoft.toml` has no SMTP row (`smtp://smtp.office365.com:587` with XOAUTH2 would be its row) | the owner of `microsoft.toml` (W5b) adds it; if `MailTransport` gains an SMTP variant, the four brand files switch to it |
| provider files from mailo's presets: stayed in mailo (the format has no place): `expected_caps` (labels, threads, archive means, folder roles, CONDSTORE/MOVE, expunge, connection budgets), Graph send/receive policy, manual IMAP/POP3 plans, local folders, `password_warning`'s host text (the provider's `app_password` kind is the carrier now), `issuer_for_server`/`issuer_named` (autoconfig issuer naming, E3 with porter-discover) | mailo's E3, as PLAN §4 lists it |
| provider files: `google.toml` has `matching` (gmail.com, googlemail.com, MX under google.com and googlemail.com) and a mail row (`imaps://imap.gmail.com:993`); `presets.rs` pins both. A provider file cannot make a row conditional, so the row is there for every client and the Google family drops it for a client that is not the person's own (`byo = true`): the claim is `Absent { UnverifiedBuild }` and mail's restricted scope is not asked | if rows ever gain a condition, the family's filter goes |
| provider files: the domains and MX suffixes of Fastmail, iCloud, Yahoo and GMX, and their hosts and ports, are from the providers' public documentation (mailo has no domain table for them; it brands by incoming host), not recorded against live servers | the owner's live smoke (PLAN fixtures) |
| `providers/yahoo.toml`: `mx_suffixes = ["yahoodns.net"]` also claims AOL addresses (AOL Mail's MX is under `yahoodns.net` since its move to Yahoo's platform) and other Yahoo-hosted domains, and the file names `imap.mail.yahoo.com`. AOL's published settings name `imap.aol.com` and `smtp.aol.com`, not Yahoo's hosts, so an AOL address likely signs in to the wrong server | the owner's live smoke: whether `imap.mail.yahoo.com` takes AOL logins. If not, an `aol.toml` (domains aol.com, aim.com; MX `mx-aol.mail.gm0.yahoodns.net` under a longer suffix) is claimed first only if `claiming` ranks the longer suffix, which it does not yet (file order), so yahoo's suffix would drop to `domains` only |
| sign-in sheet for `app_password` files (fastmail, icloud, yahoo): the generic Fixed form asks for an address and a password, and `FieldSpec` has no place to say "this provider needs an app-specific password", so the person is not told | interface ask (frozen `porter-core::sheet::FieldKind`): add `AppPassword` ("an app-specific password the provider issues") beside `Password`; the Fixed flavor then asks `AppPassword` when `auth.kind = app_password`, and sill/detent draw its label. Exhaustive matches on `FieldKind` in consumers need the arm |
| porter-infer `pick` / `admit`: the P1 request-shape rules (tools, images, structured output against what a model declares) and the `context_needed` pre-check are not applied | the routing P1 lane (research-routing-fit) |
| porter-infer `pick`: no reviewer family filter (a reviewer pick from another family than the author's) | the routing P1 lane |
| porter-infer `Why::FallbackFrom` is defined but unused: `if_unavailable = "auto"` does not fall back yet | the routing P1 lane |
| syncd: the PIM mirror and the Graph storage mirror run by default (each for a grant syncd holds), Photos only behind a switch; no WebDAV storage mirror | `Datasets` lists what the PIM supervisor (W6e) and the storage supervisor (`datasets::storage`, rig-syncd: the app folder of a Microsoft account with a Storage grant of class Files, two ways, at `$XDG_DATA_HOME/porter/storage/<account>/`, dataset `<account>/storage_app_folder`, owned by `org.quire.Files`; and, with `SYNCD_PHOTOS=on`, the two Photos datasets for a Storage grant of class Photos, owned by `org.quire.Photos`) run, and answers the refusal `NoFittingAccount` for any other name. Only a Graph or a Google Drive endpoint is mirrored (the store is read from the endpoint family of the account's candidate, `app_folder_store`; lane google-drive adds Drive's app data folder as the same dataset, and, behind the same switch, Google Photos for a `Photos`-kind grant of class Photos: `<account>/google_photos_upload`, owned by `org.quire.Photos`): a WebDAV Storage account (Nextcloud) gets no mirror, since `webdav_replica` makes no folders on a fresh account and the app folder has no WebDAV counterpart; W6d (storage families) adds its own |
| storage-webdav: the tree walk lists every folder on every poll (PROPFIND depth 1 each, no pruning by folder etag) | Nextcloud propagates a change to every parent etag, so a walk could skip unchanged folders; a generic DAV share does not, and the replica cannot tell which it has. Closes with a `Propagation` probe (compare a folder's etag across a write of ours) or a setting, when a photo library makes a poll slow |
| storage-webdav: a tree-walk anchor and the listing a sync-collection removal needs are in memory | after a daemon restart a tree anchor is `AnchorExpired`, and a sync anchor with a removal in it is too; the engine lists again and reconciles by version, so nothing known is fetched or uploaded again. Closes if listing cost shows: persist the listing in the journal (a `Replica` cannot reach it; an interface ask) |
| storage-webdav: `hashes: None` and whole-body transfers | WebDAV servers report SHA-1 or MD5 (Nextcloud `oc:checksums`) and the engine compares SHA-256 only, so an item whose etag changed is compared by fetching it; `Http` bodies are whole vectors (`StreamLimits::max_body`, 256 MiB), so a larger file needs ranged fetches by the engine or chunked upload (`chunked_upload` is `Absent`). Closes with Nextcloud's `uploads` chunking and checksums on upload, with W6f's photo sizes |
| storage-webdav: the fixtures are written from Nextcloud's and Sabre's documented answers, not recorded from a live server; the fake serves sync-collection on files although real Nextcloud does not | the owner's live Nextcloud smoke; `Behaviour::NotImplemented` + `Propagation::Up` is the Nextcloud-files shape |
| storage-webdav: a tombstone's version is the constant `deleted`, and a PUT whose answer has no `ETag` and no readable PROPFIND returns the empty version | a server that keeps a version for deletions, or one that sends no `ETag`; the next read settles the empty version by content |
| storage-graph: delta on a folder other than the drive root is documented for OneDrive personal only (Business and SharePoint answer delta for the root only), and a delta item often has no `parentReference.path` | the replica asks delta of the dataset's folder (the app folder or one below it) and builds paths from ids (folders seen in the feed, else `GET items/{id}` up to the dataset folder); a Business account's folder delta is not shown to work. Closes with a live smoke, then a root delta filtered to the folder if it fails |
| storage-graph: the folders and files seen are in memory | a deleted folder is reported alone by Graph, so the replica gives tombstones to the files it saw in it; after a restart it has seen none, so a deleted folder's files are tombstoned only by the engine's next full listing (an expired anchor), which reconciles by content. A renamed or moved folder is one change, so its files keep their old `ItemPath` until they change themselves or the engine lists again |
| storage-graph: `hashes` is `QuickXor`, which the engine does not compute | an item whose eTag differs is compared by fetching it. Closes with a QuickXorHash in syncd's `replica_hash` |
| `linked_origins` (providers/microsoft.toml) are written from Microsoft's documentation (an upload session's `uploadUrl` on `<n>.up.1drv.com`, a personal download on `<n>.files.1drv.com`, OneDrive for Business and SharePoint on `<tenant>[-my].sharepoint.com`), not observed on a live account; `*.sharepoint.com` is every tenant's, so a granted Storage app could send a link's bytes to any SharePoint tenant (no credential goes with it) | a live Microsoft account is the check; narrow or extend the list from what it answers. The pattern is host (and optional port) only: a CDN that changes its host needs a provider-file update, not a code change |
| `Tokens.OpenLinked` / `AccountsRequest::OpenLinked` are additive (`org.quire.Accounts1.xml`, one method; `VocabVersion` stays 3: a request kind, no persisted shape changed). porter-client's socket transport sends it, and the agent that answers the socket (outside porter; the test agent in `porter-client/tests/common/agent.rs` answers it `Unavailable`) must serve it like `OpenAuthenticated` | whoever owns the socket agent adds the arm; the bus path is proven (`syncd/tests/graph_bus.rs`) |
| Microsoft accounts signed in before this change hold a refresh token consented for `Files.ReadWrite.All`; only a new sign-in asks `Files.ReadWrite.AppFolder` | nothing breaks (the broader token still reaches the app folder); a user who wants the narrower consent signs in again (Reauthenticate) |
| storage-graph: the fixtures are written from Microsoft's documented Graph answers, not recorded from a live account; the fake keeps OneDrive's eTag shape (`"{ID},n"`), parent-folder changes in delta, a deleted folder reported alone, `409 nameAlreadyExists`, `412`, `410 resyncRequired`, `507`, `Prefer: odata.maxpagesize`, 320 KiB chunks | the owner's live OneDrive smoke; the fake's quota carries no `total` until a limit is set (a real drive always has one), and a delta listing omits the app folder until something changes in it |
| storage-graph: a write is one request per file (simple upload up to 4 000 000 bytes) or one session (chunks of 10 MiB), whole in memory; a session that fails mid-way is abandoned and the file sent again from the start | `Http` bodies are whole vectors; `Replica::put` takes the whole content. Closes with W6f's photo sizes, as storage-webdav's row |
| storage-gdrive: Drive documents no conditional update, so a write reads the file's `version` first and writes second | a second writer can slip in between the two requests (one round trip). The contract suite and the bus tests cannot show it. Closes if Drive's `If-Match` on `files.update` is confirmed against the live API (the replica would send the version as the ETag), or with a revision check after the write |
| storage-gdrive: the folders and files seen are in memory, and a renamed or moved folder is one change | Drive reports the folder alone, so its files keep their old `ItemPath` until they change themselves or the engine lists again (an expired anchor); a deleted folder's files are tombstoned only if the replica saw them in this process. As storage-graph's rows |
| storage-gdrive: `hashes` is `Md5`, which the engine does not compute | an item whose version differs is compared by fetching it. Closes with an MD5 in syncd's `replica_hash` (no md5 crate is in the pinned block: an interface ask for `md-5`) |
| storage-gdrive: a write is one request per file (multipart up to 4 000 000 bytes) or one session (chunks of 10 MiB), whole in memory | as storage-graph's row; a session that fails mid-way is abandoned and the file sent again from the start (the session's `308` offset query is not used) |
| storage-gdrive and the Google fake: every endpoint, query parameter, status and body is from Google's documentation, not a recorded live answer | unverified, to check when the owner's client first runs: `changes.getStartPageToken` and `changes.list` with `spaces=appDataFolder`, `includeRemoved`, `fields` (and that a page token Drive refuses is `400`, `404` or `410`: all three are `AnchorExpired`); `files.list` with `spaces=appDataFolder` and `q`; `about?fields=storageQuota` under the `drive.appdata` scope; the alias `appDataFolder` as a file id and a `parents` entry; `X-HTTP-Method-Override: PATCH` on a POST to `/upload/drive/v3/files/{id}` (`porter-http::Method` has no PATCH); the resumable protocol (`Location`, `308`, `Content-Range`, 256 KiB chunks); `403 storageQuotaExceeded` and the rate-limit reasons; `md5Checksum` absent on a folder |
| syncd: a dataset's `store` is not atomic with the engine's pre-store check | the engine re-reads the local file against the journal's fingerprint just before it overwrites (`moved_since_scan`), but a user edit in the instant between that read and `Dataset::store` is the dataset's to guard (write to a temp file and rename); `PimMirror` (W6e) does, W6f implements `store` that way |
| syncd: a dataset's `store` is not atomic with the engine's pre-store check | the engine re-reads the local file against the journal's fingerprint just before it overwrites (`moved_since_scan`), but a user edit in the instant between that read and `Dataset::store` is the dataset's to guard (write to a temp file and rename); `PimMirror` (W6e) does, `PhotoOriginals` and `PhotoMetadata` (W6f) do the same |
| syncd PIM: no flow gives syncd a Calendar or Contacts grant | the supervisor mirrors an account only when syncd already holds a grant for it (`PimGrants`, `ClientGrants` over `Accounts::find`; a consent sheet needs a parent window and a host to draw it, which a daemon lacks). Tests seed the grants; today the person would have to make them in Settings. Closes with W3e (the sheet host) plus a Settings action "Sync calendars and contacts to this computer" (or a first-run `Choose` from syncd): W3e / the detent accounts pane |
| syncd PIM: collections are found again every ten minutes, items every poll (sync-collection, else an etag walk) | a new, renamed or recoloured calendar appears within `PimConfig::rescan`; no CTag shortcut and no push; tasks calendars (VTODO) are mirrored like any calendar. Closes if the delay shows |
| syncd PIM: collections are on the endpoint's origin only | the relay dials the endpoint's origin; a home set on another host (rare; iCloud shards) is an `Unreadable` discovery. Closes with the iCloud provider lane (W6g) |
| syncd `Sync1.Resolve`: a dataset registered with `Access::default()` (no owning app) cannot be settled by anyone (Settings and the daemons see it and are `Denied`); a call waits for the dataset's driver, which answers between cycles (a cycle stuck on the network delays it) | every dataset syncd runs today names its owners (PIM, Photos); if an ownerless dataset appears, decide whether Settings may settle it. `Sync1` has no owner notion of its own: ownership is `Access.owners`, the rule `Datasets`/`Status` use, narrowed to the app itself |
| syncd: the network seam (`watch::Receiver<Network>`) has no NetworkManager reader | `main` passes `Network::Unmetered` to both supervisors (PIM and storage); the lane that reads `org.freedesktop.NetworkManager` `Metered` (never in tests) feeds the one channel |
| syncd storage: revoking a grant stops the mirror and keeps its files and journal; only `AccountRemoved` deletes them | PIM deletes its mirror on revoke (a read-only copy of the server), but the app folder is two-way and may hold edits never uploaded, and the Photos library holds the person's own imports; a new grant picks the folder up again where it was. If the owner wants revoke to wipe, it is one `remove_dir_all` in `StorageSupervisor::retire` | the owner's call |
| syncd storage: `FolderDataset` hashes every file on every scan, lists no empty folder, follows no symlink and watches nothing | a poll cycle reads the whole folder (the scan is the SHA-256 of each file, chunked, on a blocking thread); a local change is found at the next poll (`Settings::poll_base`), not at once; OneDrive is case-insensitive and the local folder is not, so two local names differing in case are one remote name. A change feed from inotify, and a size and mtime shortcut for the scan, close the first two | the Files app lane |
| syncd storage: the grant that makes a mirror is class `Files` (app folder) or `Photos` (Photos) with `Usage::Background`, asked by `Accounts::find` as `org.quire.Sync` | accountd's settings module now makes and takes it back (`accounts.<id>.sync.files` and `.sync.photos`, lane reauth-grant, section below); detent's accounts pane does not draw those rows yet (its `classify` keeps only `.service.` toggles), and the PIM grants (Calendar, Contacts) still have no offer | detent: the ask in the reauth-grant section; PIM: W3e / the detent accounts pane |
| porter-fake-servers: `FakeGraph::bind_issued` accepts exactly the access tokens its `IssuerHandle` has minted (or planted with `seed_access_as`) and not revoked; its links stay on its own origin | `bind_linked` keeps the fixed bearer and the second origin (the relay's `OpenLinked` path); a rig scenario that needs links on another origin has to declare it in a provider file; the mail fakes (`Accounts::bearer`) still take a fixed list, so an XOAUTH2 login with an issuer-minted token is refused | a scenario that needs it |
| porter-rig (test tooling, `publish = false`, never in `dist/`, check-boundary lists it): `porter-rig-servers`, `porter-rig-client`, `porter-rig-secrets` | the control endpoint is plain HTTP on loopback with no authentication (the rig is for a jail); `rig.json` and the CLI are the scenarios lane's contract (see the lane report); `porter-rig-client open` is not tested against a real inferd here (the bus tests cover the other commands against the real accountd and syncd's `Sync1`); the client names itself only as a Flatpak scope, and its role is the daemons' own caller table | the scenarios lane, when it needs a native app or another role |
| syncd: SQLite calls run on the async runtime's thread (current_thread in main) | each journal write is one short transaction; W6f's file work already runs on blocking threads, the journal's does not: a 1 000-photo cycle is one transaction per item, each synced to disk, so a cycle is slow under load (acceptance 5 takes about a minute on a busy machine); batch the journal ops of a page if it matters |
| Photos (W6f): no app, no grant flow | the daemon wiring is done for Graph (`StorageSupervisor`, rig-syncd): with `SYNCD_PHOTOS=on` a Storage grant of class Photos on a Microsoft account runs both datasets over `Photos/Originals` and `Photos/Metadata` of the app folder (`graph_replica` makes the folders), the library at `$XDG_DATA_HOME/porter/photos/<account>/`, the supervisor's `photos_library(account)` being where the Photos app will import. Still open: no Photos app, nothing makes the grant (Settings, or W3e's sheet host), and a WebDAV account has no wiring (it needs the folders made on a fresh account) |
| Photos: deleting is a mark, the original stays; no purge | `Mark::On` on `deleted` hides a photo; the original and its replica copy stay, so another device can still restore it. A purge (delete the original on every device after a retention time) needs a retention rule from the owner and the Photos app's trash |
| Photos: an original is one body (`Dataset::read` returns `Blob`, `Http` bodies are whole, 256 MiB cap) | videos and RAW files beyond the cap need ranged or chunked transfer: storage-webdav's chunked upload (see its row) and a streaming `Dataset` read, in the lane that closes that row |
| Photos: a device's name is chosen by the caller of `PhotoLibrary::open` and kept in `<library>/device` | the storage supervisor derives it (`d` and twelve hex digits of a hash of the account, the time and the pid), once: the library keeps the first. Two machines with one name would still write one remote manifest and conflict, which the engine would store, not hide |
| inferd hosted: Anthropic's own account reaches nothing (wire `AnthropicMessages`), so Claude goes through OpenRouter | the stoker Messages API lane after the demo; then `cloud::models::WIRES` gains it |
| inferd hosted: the key is fetched for each turn (`ResolveKey`), so a `GrantScope::Once` grant works for one turn only; the account's provider comes from `provider` in the verdict's details (accountd sends it), with the account id's stem as a fallback for an older accountd | a `ResolveKey` per session if Once grants should last a session (design/31 §5.4 decides) |
| inferd hosted: only chat and task turns run on a hosted model; a structured reply is sent as the provider's constraint but not validated or repaired, and embeddings, speech and computer use are refused `Unsupported` | the lane that gives `structured::shaping` a model that is not a `LocalModel` |
| inferd hosted: stoker's `Flavor` has no Gemini, Moonshot or OpenAI row, so those use `LiteLlm` (a usage chunk asked for, `reasoning_effort`); gpt-6-luna's "reasoning off with tools" is a body change in inferd's transport (`reasoning_effort: "none"` for OpenAI, `reasoning: {effort: "none"}` for OpenRouter, the second from OpenRouter's docs, not exercised against it); `temperature` is left out unless the app chose a sampling | stoker flavors and a `Reasoning` value for "none"; a recorded fixture per provider |
| inferd hosted: stoker's hyper client answers a TLS target `HttpError::Tls`, so `cloud::transport` is a copy of its exchange with a rustls handshake; inferd gains the `model-wire` edge (Cargo.toml, `scripts/check-boundary.sh`, ARCHITECTURE section 1: applied in the AI2b branch) | stoker's `HttpClient` speaking TLS, then delete `cloud::transport` |
| inferd spend: days and months are UTC, the four caps (`ai.spend.{account,app}_{daily,monthly}_cents`) default to none, and the estimate before a turn is a thousand tokens each way; `spend.json` is rewritten whole on each reply | the owner's call on defaults and local time; a calendar crate |
| inferd picker: a hosted choice is `cloud/<model>` (the account is whichever the app is granted), the Menu has only a label per word, so family and capabilities ride in the label text ("Name (Company, tools, images, reasoning)") and "Add an account to use" ends it when no account of the person's reaches it. The menu is sectioned by the schema's `groups` (quire `KeySpec.groups`): each hosted choice under its company (`company_of` its catalogue family: Anthropic, Google, Moonshot, OpenAI), each on-device model under "On this computer", "" and Automatic ungrouped on top; a model of the engines that runs in the cloud and is not a curated entry stays ungrouped | a `KeyKind` that carries tags (quire) for detent's filter |
| inferd.toml: the old `[policy]` and `[tiers]` tables are still read (one way; the `[ai]` rows at their settings paths win field by field); `Set` rewrites the file from its parsed form, so comments in it are lost (no `toml_edit` in the pinned block) | `[policy]`/`[tiers]` go one release after detent writes the `ai.*` paths; comments are kept when quire pins `toml_edit` (an interface ask) |
| inferd `Snapshot.gpu` refreshes only on `Want`, so a cold swap cost reads `Fits` until then | a refresh on `Prepare` (inferd) |
| porter-infer `route` with an empty picker row does not exclude `NonCommercial` models | the routing P1 lane |
| inferd `Why::Evicted` names only the first victim of a swap | the routing P1 lane, if a swap ever evicts more than one |
| inferd engine start failures (engine-start): a failed start is a typed `startup::Cause` (exit status and the last stderr line, never ready, socket path too long / not a socket / unusable, program cannot be run, no room), carried by `supervise::Failed` and `serve::EngineFailed` and logged (error for setup causes, warn with the last 40 lines / 8 KiB of stderr, control characters stripped). The session is still told `Failed(NotReady)` (and a route during the pause `Unavailable`): `ModelError` is `Copy` with no room for text, so the cause reaches inferd's log and tests, not the client. A waiter is failed at the engine's first exit before ready, not at the 180 s readiness timeout; the machine goes on in the background (3 attempts, backoff 2 s then 4 s), each failing the waiters it has; after the last, requests are failed at once and the engine reads `Unavailable` for 30 s (the backoff cap), then `Loadable`, and the next request starts it from attempt 1; no request restarts it sooner. Before every spawn a socket path of 108 bytes or more is refused, a stale socket is removed, anything else there is refused and kept | the cause on the wire: `ModelError::NotReady` carrying a short text (an ask on porter-infer, consumer checks) |
| inferd engine processes (engine-start, inferd-sigterm): each engine leads its own process group and has `PR_SET_PDEATHSIG(SIGKILL)` (`hosts::group`; the only `unsafe`, one `pre_exec` of one `prctl`). Stop, eviction and a start that never becomes ready signal the group (SIGTERM, 5 s, SIGKILL) before the exit is reported; an engine that exits by itself has its group swept; dropping the host (inferd returns or unwinds) kills every group still running. Covers by case: **SIGTERM / SIGINT** (`systemctl --user stop`, a terminal) `inferd::shutdown` (tokio `signal`, added on inferd's own line, not the workspace pin): the bus name is released, `HostCloser::end_all` stops every engine through the group stop and the host starts nothing more, exit 0, all within `shutdown::BOUND` = 15 s (two 5 s graces and margin, under `TimeoutStopSec=20`); a second signal or the bound kills every group at once and exits 1 (tests `crates/inferd/tests/shutdown.rs`: real binary on a private bus for the handlers and the name, a stand-in over the real `ProcessHost` with a `SIGTERM`-ignoring grandchild for the group); **normal exit** the drop; **crash/unwind** the drop (a panic that aborts does not run it, then as SIGKILL); **SIGKILL of inferd under the unit** PDEATHSIG ends the direct child and the unit's cgroup ends grandchildren (`dist/inferd.service` has `KillMode=control-group`, no `SendSIGKILL=no`, `Delegate` or `ExecStop`, so a stop or restart ends the cgroup, within `TimeoutStopSec=20`); **inferd outside a unit** (tests, harnesses) a SIGKILLed inferd leaves grandchildren (vLLM `EngineCore`, the resource tracker) alive, holding GPU memory, until someone ends them. What still relies on the unit: SIGKILL of inferd (the OOM killer, the stop timeout) | for the SIGKILL-outside-a-unit case, a subreaper or a transient unit per engine (the systemd host of models section 3.9) |
| inferd attached engines (I3): `[engines.attached."<catalog id>"]` in `inferd.toml` (`socket` or loopback `url`, optional `key_file`, required `where` = `this-device` \| `my-network`) names an engine the person already runs (vLLM on a lab machine behind an SSH tunnel) and must name a catalogue id whose `serving` is `attached` (stoker a30bee8: `Serving::Attached` gives the wire `engine`, the `served_name` a request names and `/v1/models` must list; a `launched` entry or an unknown id is `AttachedError::NotAttachable` / `NotInCatalogue`, and a catalogue attached entry with no such table is simply not routable, never spawned; the catalogue's `locality` is not the routing class: `where` is, and `catalog::local_claims` makes no claim for an attached entry); inferd never spawns, evicts or kills it (it is in no supervisor, no warm budget, no eviction order, and `inferd::shutdown` has no handle on it; tests: a fake server process is alive after an eviction pass and after SIGTERM to the real binary). Covered: a chat over a Unix socket and over loopback TCP through the catalogue's bridge; readiness is `GET /v1/models` listing the catalogue's served name (`NotReady`: `SocketMissing`, `Refused`, `Unauthorized` (401/403), `ModelAbsent { served }`, `KeyFile`, `Unanswered`), looked at when a session opens (`Engines::offer`, which every route, `Prepare` and `Availability` goes through) and again by `want`, never polled, carried as `startup::Cause::Attached` to inferd's log (one line per change) and tests; the bearer is read from `key_file` at each connect (refused unless the file is a regular file, owned by the user, with no group or other bits; the token is a `Secret` and is in no log line or `Debug`); a non-loopback `url`, `https`, a missing `where`, both or neither of `socket` and `url`, a relative path and an id the catalogue lacks refuse the file (`config::AttachedError`; the last at daemon start); `this-device` is `Locality::OnDevice`, `my-network` is `Locality::LocalNetwork` (an on-device floor refuses it with the usual `RequiresCloud(class)` unless the new row `ai.attached.my_network` is `on`, which routing reads as every `on_device` floor being `local_network`; a floor that already says `local_network` admits it either way), billing `Free`, no accountd grant, never cloud for spend or `ai.local_only`. Not covered: TLS (a socket or loopback only, `http` only; `[::1]` is not accepted), the tunnel's lifetime (the owner's: nothing starts, restarts or watches `ssh`), a Unix socket or key file the unit's sandbox cannot reach (`inferd.service` has `PrivateNetwork=yes`, so a `url` needs the network drop-in; a socket under `%t` works), an attached engine that is not a chat model (embeddings, speech and computer use on one are untested), a `key_file` owned by another user (the check is there, untested: it needs root), attached engines added or changed without a restart (read at start, as `[engines]` is), the picker listing a `my-network` model under "On this computer" (it is listed ungrouped, as `local/<id>`), and a catalogue id attached while the same id is also run by inferd (the attached one wins and the other is not started) | the owner's asks; picker grouping "On my network" if wanted |
| `AccountService::add_and_allow` (built, tested) is not called by `Choose` | `choose` answers `NoFittingAccount` when nothing fits, and the app opens the add sheet itself with `AddAccount`, which carries no need and so grants nothing. "Add, and allow" in one step needs the `Choose` flow (or an `AddAccount` that carries a need) to call `add_and_allow`; closes where the app-facing add path is decided (W3d's bus method or mailo E1) |
| Generic IMAP, POP3 and typed SMTP check no password at add time; one password only | `porter-families` has no IMAP client (the relay's is porter-proxy's, W3g), so a wrong password is found by the first relay or `open` and sets `NeedsReauth`; a separate outgoing password (`SecretPurpose::OutgoingPassword`) is not asked for either. Closes with an IMAP login probe in the family, or a decision that the relay's first refusal is the check (W3g) |
| porter-families Nextcloud discovery falls back to conventional DAV paths | The fake Nextcloud (W2e) answers no `current-user-principal` or `calendar-home-set`, so the conformance tests walk the fallback (`/remote.php/dav/calendars/<user>/`, `/addressbooks/users/<user>/`); the principal walk is written over porter-dav's parsers but not exercised end to end. Closes when the fake serves principals, or at the owner's live Nextcloud smoke |
| `HyperHttp` trusts the platform's roots only | The pinned `webpki-roots` fallback is not used: its licence (CDLA-Permissive-2.0) is not in `deny.toml`'s allow list. A host with no system roots (a minimal container) can trust a private CA through `HyperHttp::with_extra_roots` and nothing else. Closes when the owner allows the licence and the fallback joins `with_extra_roots` |
| Login Flow v2 does not follow a redirect | `HyperHttp` never follows one, so a server whose `/index.php/login/v2` redirects (a canonical host) fails `Unreadable`; the person types the canonical address |
| `AccountService::rediscover` (built, tested against the Microsoft fake) has no caller | accountd decides when discovery runs again (a settings action, a start-up sweep, an account going `NeedsReauth`); until then claims are refreshed only by adding or signing in again. It discovers, then opens a session and stores its `renewed()` credential |
| Microsoft `discover` hands a rotated refresh token to the next `open` through a table inside `MicrosoftProvider` (keyed by account and the token it was given), because `Provider::discover` returns claims only | an interface change if another family needs it: `discover` returning the claims with an optional renewed credential, which would delete the table. `Provider::discover` also cannot refresh an account's endpoints (a Nextcloud that moved keeps its stored URLs) |
| Nextcloud finds its server for `discover`, `revoke` and a sign-in again from the first endpoint under `/remote.php/` or `/index.php/` | an account whose endpoints were all edited away has none to read: `discover` and `revoke` answer `Unreadable` and a sign-in again asks for the server like an add |
| `WebUrl` accepts `https`, or `http` to loopback only | a Login Flow v2 server that answers a plain-`http` login page on another host fails `Unreadable` (it is refused where it enters, before any sheet shows it) |

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

**`SocketTransport`/`SocketSession`** (feature `socket`; Unix sockets, and a named pipe on
Windows (`socket/windows.rs` over tokio's pipe client at latchkey's pipe name; no descriptors, so
`open_authenticated`/`open_linked` are `Unreachable` and a frame naming attached images is
`Malformed`; compiles for `x86_64-pc-windows-msvc`, never run); without the feature `call` and
`open_with` are `Unreachable`):
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
- **`Adopt` is removed** (owner decision, drop-adopt lane): accountd holds every account and an
  app's older keyring sign-ins (mailo's `service=mailo` entries) are not moved; the person signs
  in again. Gone: `AccountsRequest::Adopt`, `LegacyRef`, `LegacyItem`, `AccountsReply::Adopted`,
  `Accounts::adopt`, `Manager.Adopt` (and its introspection), accountd's `legacy` module and
  `[adopt]` table (`/etc/porter/accountd.toml` is no longer read), porter-service's `adopt`.
  `AuditEvent::Adopted` stays as a variant nothing writes, so an `audit.jsonl` an earlier daemon
  wrote still reads. `VocabVersion` stays 4.
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
3. **mailo, E2**: dropped with `Adopt` (the person signs in again).
4. **sill (W3e)** serves `org.quire.AccountsSheet1` (`dbus/org.quire.AccountsSheet1.xml`): views
   are `porter_core::sheet::SheetView` as JSON, inputs `SheetInput` as JSON (a typed password
   rides in `FieldValue::Secret`). Only accountd's connection may `Open`.
5. **quire `ds-shell::accounts`** maps `SheetView` states: `Consent`, `Providers`, `SignIn`
   (fields are kinds; the UI words them), `BrowserWait`, `ShowCode`, `Review` (with the
   add-and-allow `allow` app), `Working`, `Failed`, `Done`.
6. **docket and cua `check-boundary.sh`** (PLAN §5): `Accounts::open_authenticated` joins the forbidden calls in agent crates.

## Open

- inferd's stoker edges are by sibling path (`model-provider`, `model-catalog`, `engine-supervisor`, `model-http`, `model-openai-compat`, `vision-prep`, `cua-parse`, `speech-provider`), like `cua-action`; the pinned git revs replace them with quire's block. `cua-session`, `cua-vendors` (for `StepResult`) and `model-extract` are edges since W5; `speech-host-client` is since the voice-inferd lane (inferd's `SttBackend::SpeechHost`).
- `InferSession` and its `SessionError` live in porter-infer (re-exported by porter-client), not in porter-client as models §3.8 words it: `porter-fake` implements the trait for `FakeInferSession` and must not depend on the client. `next()` returns `Result<InferEvent, SessionError>` (`Closed` after the daemon ends the session) where the spec says `InferEvent`.
- `DroppedAction`, `DropReason` and `SafetyHint` are named by the specs but not defined; porter-infer defines them (`SafetyHint` mirrors stoker's `SafetySignal`: `RequireConfirmation`, `Blocked`; both only add asks).
- The picker is a plain list (QUESTIONS V4): no recommended or best row and no ranking language; `Tier` stays because apps ask by tier and `Open` takes one. `picker_rows` breaks ties by the catalog's order, not by label.
- inferd serves `SettingsModule1` at `INFERENCE_SETTINGS_PATH` for the `ai.model.<kind>.<tier>` picker only; the other `ai.*` rows are file rows of `dist/inferd.settings.toml`. `<kind>` becomes `<slot>` when C1 renames it. A `Set` and a changed file reach the next session (polled every 2 s, and on `Rescan`); a session already open keeps its model. `Role` gained `Settings`, from `[callers] settings = [<app>]`.
- `ai.floor.voice` (default on device) has no design/22 row yet; the quire agent adds `voice.*` and `ai.*` rows.

- Proxies are not run against skeletons (needs a zbus p2p test); closes with the codec.
- accountd's `main` resolves `$XDG_STATE_HOME` (`accountd::paths`), loads `FileStore` (refusing a `StoreFault`: the daemon exits and leaves the file alone) and passes it and `FileAudit` to the service. The caller's pid is read from the bus's `GetConnectionCredentials` and not from the bus's `ProcessFD`, so a pid that is reused between the bus's answer and the `/proc` read names the wrong process: `ProcessFD` is a later hardening (it needs zbus's credentials `ProcessFD` and a pidfd-based cgroup read).
- accountd reads its caller tables from `/etc/porter/callers.toml` and `$XDG_CONFIG_HOME/porter/callers.toml` (the user's rows win), provider files from `/usr/share/porter/providers`, `$XDG_DATA_HOME/porter/providers` and each `--providers DIR`, and the user's `clients.toml` from `$XDG_CONFIG_HOME/porter/`. `ACCOUNTD_PROC_ROOT` (a `/proc` fixture for the jailed tests) works only in a build with the non-default `test-proc-root` feature, and the build then logs one line naming it.
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

## Lane w6a-syncd (syncd, the engine)

Lane `w6a-syncd`, branch `w6a-syncd` from ea27908. syncd was a skeleton; it is now a library and a
daemon. No `todo!()` was left behind (none in syncd before or after).

- **Journal** (`syncd::journal`, rows and pure rules in `porter_sync::journal`,
  `journal_reconcile`): SQLite per dataset at `$XDG_STATE_HOME/porter/sync/<account>/<dataset>.sqlite`
  (`<account>` is the object-path segment, `porter_core::object_segment`, so `AccountRemoved` names
  the directory). Tables `items`, `anchors`, `tombstones`, `conflicts`, `schema_migrations`;
  `MIGRATIONS[n]` takes version n to n+1, a file from a newer syncd is refused, a test opens a
  hand-written v1 file and one brings it to a v2. `rusqlite` is quire's pinned line verbatim
  (`bundled-sqlcipher-vendored-openssl`, used unkeyed): `cargo deny check licenses` passes with it.
  Every change is one transaction (`Journal::apply(&[Op])`).
- **Engine** (`syncd::engine`): a cycle is resume, record the local scan, pull, push, compact. Item
  states name the step that finishes them (`fetching`, `discarding`, `pending_upload`,
  `pending_remove`, `conflicted`), so a crash between any two writes is finished by the next cycle:
  `engine::tests::cut_points` cuts before every journal write of a mixed scenario (remote edit,
  delete and create; local create, edit and delete), with and without replica hashes, and the sides
  must converge to the clean run's state. The local scan is recorded before the pull and a fetch
  re-checks the local file, so a local edit is never overwritten. `AnchorExpired` lists in full and
  reconciles by content hash (a deleted and re-created item with the same bytes is re-bound, not
  fetched); a replica that reports no SHA-256 is compared by bytes. A refused write is a stored
  conflict (`Changed`, `Deleted`, `Exists`), never an overwrite; the same bytes already there are
  adopted. Tombstones are kept until the feed shows them (or a full listing proves them gone),
  then compacted.
- **Datasets**: `syncd::dataset::Dataset` (scan, read, store, discard; fingerprints are SHA-256 hex,
  `dataset::fingerprint`), `MemoryDataset` behind feature `testing`. `DatasetKind` has no PimMirror
  variant (W6e names its dataset by slug through `DatasetId`; the frozen enum is not touched).
- **Scheduler** (`syncd::scheduler`, pure): inputs are the clock, the network and each dataset's
  history; push waits a batching window then runs, poll doubles its interval per idle cycle, a
  failure backs off exponentially and never sooner than `Retry-After`, equal jitter from a seeded
  `SplitMix64`, `coalesce` batches wake-ups, metered/offline/user pause hold. `syncd::driver` is the
  loop around it.
- **Sync1** (`syncd::service`): the four methods and both signals of the frozen XML (and `Resolve`, added after: see Interface asks, item 2), callers by
  `ProcCallers` (the `test-proc-root` feature, `SYNCD_PROC_ROOT`, a dist test and a check-boundary
  row as accountd). A dataset is named `<account>/<dataset>`; apps see the datasets they own,
  Settings and the porter daemons all; an unknown or invisible name is the refusal
  `org.quire.Accounts1.Error.NoFittingAccount`, a malformed one `InvalidArgs`, an unnamed sender
  `AccessDenied`. `Status`: `anchor_age` (x), `pending`, `conflicts` (t), `paused` (b), `quota`
  (`STATUS_KEY_QUOTA`, a{sv} with `used` and `total`, both t). Signals are unicast to callers who
  may see the dataset.
- **AccountRemoved** (`syncd::removal`): wipes `<state>/porter/sync/<account>` and
  `<data>/porter/vdir/<account>` and stops the account's datasets. accountd unicasts only to
  connections that have called it, so syncd calls `Grants.List` at start and whenever
  `org.quire.Accounts1` gets a new owner. W6e: the mirror directory under `vdir/` must be named by
  the same account segment.
- `dist/syncd.service` (AF_UNIX only; writes only the journals and mirrors) and
  `dist/dbus/org.quire.Sync1.service`. check-boundary: the syncd row gains `porter-core`, plus the
  `test-proc-root` leak check. `/etc/porter/callers.toml` needs a `[[caller]]` row for
  `syncd.service` with role `porter_daemon` (packaging; not a file of this lane).
- porter-sync gained `journal` and `journal_reconcile` (no new dependency; not consumer-used).

### Interface asks from W6a

1. `porter-dbus` `sync.rs`, doc comments only (the XML is unchanged): `datasets` returns
   "the dataset names the caller may see, as `<account>/<dataset>`" and `status`, `pause`,
   `resume` take such a name.
2. DONE (lane sync-resolve, owner decision 2026-10-07): `Sync1.Resolve(dataset: s, conflict: x,
   how: s)` settles a conflict (`how` is `keep_local` or `keep_remote`; `conflict` is the `number`
   key of the `Conflict` signal, which now carries it). Errors: `InvalidArgs` (bad `how`),
   `org.quire.Accounts1.Error.NoFittingAccount` (no such dataset for the caller),
   `org.quire.Accounts1.Error.Denied` (sees it, does not own it), `org.quire.Sync1.Error.NoSuchConflict`
   (unknown or already settled). The driver settles it between cycles and runs the next at once.
3. `DatasetKind` has no variant for the PIM mirror (W6e); datasets are `DatasetId` slugs in syncd,
   so nothing is needed unless the owner wants it in the frozen enum.

## Lane w6e-pim (syncd, the PIM mirror)

- `crates/syncd/src/datasets/pim/**`: `PimMirror` (the `Dataset` of one collection: ledger of what
  it stored, atomic temp-file-and-rename writes, UID file names, the open-time heal), `discover`
  (endpoint, principal, home set, collections with `displayname` and `calendar-color`, over
  porter-dav), `plan` (directory names and dataset slugs, clash-proof and order-independent),
  `relay` (`PimDial`, `pim_http`, `pim_replica`: relays to the endpoint, a replica over the
  collection's own URL, which `webdav_replica` cannot express), `grants` (`PimGrants`,
  `ClientGrants`), `mirrors` (`AccountMirrors`: one account's collections as engines and drivers
  in the hub) and `supervisor` (`PimSupervisor`, `PimConfig`). `main` starts the supervisor.
- vdir layout, `$XDG_DATA_HOME/porter/vdir/<account>/<collection>/`: `<account>` is
  `object_segment(account)`, `<collection>` the last segment of the collection's URL made safe, plus `-contacts` for an address book (so a calendar and an address book of one name never share a directory);
  `<uid>.ics` or `<uid>.vcf` (the item's UID when it is a plain name, else the server's file name,
  else that plus a hash), `displayname`, `color` (calendars only). Sync1 names a collection
  `<account>/pim_cal_<dir>` or `<account>/pim_card_<dir>`; its journal is
  `$XDG_STATE_HOME/porter/sync/<account>/<slug>.sqlite`.
- A grant that disappears (revoked, account removed) stops the account's mirrors and deletes
  their directories and journals; an accountd that cannot be asked changes nothing
  (`AccountdUnavailable`). `AccountRemoved` still wipes the account's directory at once
  (`removal::wipe`, tested against the live mirrors).
- `Dataset::direction()` (`Direction::{TwoWay, PullOnly}`, default `TwoWay`): a pull-only cycle records no local change and pushes nothing, and `upsert_known` never makes an edit a conflict. `PimMirror` is `PullOnly`, so a local edit of any size is overwritten at the item's next server change and the in-memory write-back cache was dropped; a file deleted or damaged locally is no server change, so `PimMirror::open` still queues it to be fetched again at the next start.
- Additive outside the owned paths: `Hub::forget` (one dataset), the fake DAV tree's principal,
  home sets, `Principal`, `CollectionMeta` and `Tree::make_collection`/`set_meta`, and the
  Nextcloud handle's `add_calendar`, `add_addressbook`, `set_collection_meta`, `delete_item`,
  `delete_collection`. `syncd` gains `porter-dav` and `porter-client/dbus` as dependencies and
  `porter-provider` as a dev-dependency.
- The fake Nextcloud's `expire_sync_tokens` only stales tokens older than the server's counter,
  so a test that wants an expiry makes a change first.
- sill: `calendar.sources = auto` needs a new place kind to read `porter/vdir/*/*`; the diff is in
  the lane report (sill is Whopper's repo). sill reads `.ics` only and ignores the `displayname`
  and `color` files; the calendar's name in the widget is its directory name.

- AI1a (local runtimes as accounts): inferd's `[probe]` table (`ollama`, `llama_cpp`, `lm_studio` port lists, `every_s`, `longest_s`; a runtime with no ports is not asked) is the one place a daemon looks at a loopback port. `tests/proc_root.rs` writes it empty so the spawned daemon never asks a port of this machine. A probed account's id is its provider's id (`ollama`, `llama-cpp`, `lm-studio`; `providers/llama-cpp.toml` and `providers/lm-studio.toml` were added for the last two, additive in `SHIPPED_FILES` and the count in `shipped_files.rs`). `porter-service` gains `AccountService::report_local` (new file `local.rs`, `LocalFault`, `MAX_LOCAL_CLAIMS`), the only owned-path exception; `porter-fake-servers` gains `Bind::Port`, `FakeModels::bind_on`, `say` and `chats`, and a chat-completions answer on both wires.

## Lane w6c-graph (storage-graph, the Microsoft Graph replica)

Lane `w6c-graph`, branch `w6c-graph` from 11651d4; its asks are closed by lane `graph-relay` (below).

- **`storage-graph`** (new crate; depends on porter-core, porter-http, porter-sync and storage-webdav,
  none changed): `GraphReplica<H: Http>` is porter-sync's `Replica` over a folder of OneDrive's app
  folder (`/me/drive/special/approot`). `changes` is a delta query (`Prefer: odata.maxpagesize`), the
  next or delta link as the anchor, a `410` (`resyncRequired`) or a link from another origin
  `AnchorExpired`; `fetch` is `GET items/{id}/content` with `Range`, following the one redirect
  Graph answers with; `put` is one PUT up to `SIMPLE_MAX` (4 000 000 bytes, Graph's simple-upload
  limit) and an upload session past it, chunks of a multiple of 320 KiB (`Uploads`, default 10 MiB);
  a new item carries `@microsoft.graph.conflictBehavior=fail` (`409`), a changed item or a removal
  `If-Match` with its eTag (`412`), and the refusal is read back into a `Conflict` (`Changed`,
  `Deleted`, `Exists`); `quota` is `GET /me/drive` `quota` (a `total` of 0 is no limit);
  `features()` is read-write, poll, quota reported, `AppFolder`, `QuickXor`, ranges, chunked upload.
- It reuses storage-webdav's `StreamHttp`, `Dial`, `Clock` and `DELETED` (re-exported) and copies
  nothing of it; its error classes, URLs and wire reading are its own. The bearer is added by
  accountd's relay; a test asserts the replica sends no `Authorization` itself.
- **syncd**: `syncd::graph::graph_replica` (the relays of `syncd::webdav::RelayDial`, which gained a
  `RelayDial::new`); no daemon wiring beyond it (FINDINGS row above).
- **porter-fake-servers**: `graph` (`FakeGraph`, `GraphHandle`, `Knobs`): a drive of items with
  OneDrive's eTags, delta with tokens and skip tokens, children, content with ranges and
  redirects, simple and session uploads with 320 KiB chunks, quota, throttling, and a bearer
  check; additive.
- Tests: the contract suite of storage-webdav (the same nine cases) over `MemoryReplica` and over
  `GraphReplica` in three setups (simple upload, every file in a session, redirected downloads);
  documented-shape fixtures through a scripted `Http`; the replica against the fake (dataset folder
  made and filtered, deleted folder, upload threshold and chunks, conditions on sessions, a remote
  edit or delete between listing and write is a `Conflict`, anchor expiry with zero writes,
  redirect and range, throttling, no credential from the replica); and
  `syncd/tests/graph_bus.rs`: syncd, the real accountd (a fake provider minting the bearer) and
  the fake drive on a private bus, with `Sync1.Status` showing the quota, a large file in a session,
  and an expired delta token uploading nothing.

- **Lane `graph-relay`** (branch `graph-relay` from a907c2a) closes the asks below. Tests added:
  `porter-provider` `spec::linked::tests::{a_pattern_names_hosts_by_label_and_the_default_port_unless_it_says_one, patterns_that_would_name_too_much_are_refused}` and
  `shipped_files::only_microsofts_storage_row_declares_linked_origins_and_asks_the_app_folder_only`;
  `porter-proxy` `http1::tests::an_anonymous_relay_adds_no_credential_and_drops_the_apps`;
  `porter-fake` `service::a_linked_relay_dials_only_an_origin_the_provider_file_declares_and_presents_nothing`;
  `storage-graph` `route::tests::the_home_origin_and_the_others_are_kept_apart_and_a_linked_client_is_made_once` and
  `tests/linked.rs` (`a_large_file_goes_up_and_comes_down_through_the_second_origin`,
  `no_bearer_and_no_credential_of_any_kind_is_sent_to_the_linked_origin`,
  `an_origin_the_declaration_does_not_name_is_never_reached`; the fake serves the links on a
  second loopback origin that refuses any `Authorization`: `FakeGraph::bind_linked`);
  `syncd/tests/graph_bus.rs` (now a real `graph` endpoint and `OpenLinked`;
  `a_linked_relay_reaches_a_declared_origin_only_adds_no_credential_and_strips_the_apps`);
  `accountd` `roles` (an Agent is refused `OpenLinked`); `porter-core` round-trip and slug rows.

### Interface asks from W6c (closed by lane graph-relay, branch `graph-relay`)

1. **Closed.** `Family::Graph.relay_protocol()` is `Some(EndpointProtocol::Http)` and
   `Family::serves` has `(Family::Graph, Storage | Photos)`; syncd's bus test now uses a real
   `graph` endpoint (and the audience `graph`) and the provider file's real family.
2. **Closed.** Linked origins. A provider-file row declares `linked_origins = [..]`
   (`porter_provider::LinkedOrigin`: `host`, `*.suffix`, optional `:port`; a wildcard needs two
   labels after `*.` and never matches the suffix itself; no port means the scheme's default).
   `Tokens.OpenLinked(grant, origin) -> h` (new method; the frozen XML gains one member, nothing
   else moved) and `AccountsRequest::OpenLinked { grant, origin }` open a relay with
   `RelayAuth::Anonymous` (new variant: the HTTP relay adds no `Authorization` and still drops the
   app's own; IMAP, SMTP and ManageSieve refuse it). `AccountService::open_linked` refuses
   (`EndpointNotGranted`) any origin the account's provider file does not declare for the grant's
   kind, one with a path, another scheme than the family's endpoint (no `https` to `http`
   downgrade), and a plain host that is not loopback; the audit line is `ProxyOpened` with the bare
   origin, never a link's path (it carries the secret). `porter-client`: `Accounts::open_linked`,
   `Transport::open_linked` (default `Unreachable`). The replica uses it through
   `storage_graph::Routed` (the drive's origin to the authenticated relay, any other origin to a
   linked one, at most 8 kept) and `syncd::graph::LinkedDial`.
3. **Closed.** Storage over Graph asks `Files.ReadWrite.AppFolder` (porter-families microsoft
   `scopes.rs`) and the provider row says `scope = "app_folder"`.
4. **Closed.** `FakeProtocol::Graph`; no consumer names `FakeProtocol` (grepped almanac, docket,
   cua, sill at origin/master), so it is additive for them. `FakeGraph::protocol()` answers it.

## Fill voice-inferd: Transcribe over the speech host

Lane `voice-inferd`, branch from 0941881. porter-client `Transport::prepare`; inferd's speech to text.

- **`Transport::prepare(need, class, tier, options) -> Result<Readiness, TransportError>`**, over
  `Inference1.Prepare` (`DbusTransport`) and through `SessionHost::prepare` (`InProcess`); `AnyTransport`
  forwards it. Both traits have a default body (`Unreachable`), because almanac, docket, cua, sill and mailo
  have `impl Transport` of their own (checked by grep, read only), so none of them changes. `SocketTransport` keeps
  the default (the latchkey socket carries `Open` only). `Accounts::prepare` is the app-facing call. The bus answers a
  slug: `ready`, `loading`, `loadable`, `downloading` (its progress is not on the bus, so it reads
  `Downloading(Permille(0))`), `downloadable`, `unavailable`; any other slug is a refusal (`needs_grant`, `denied`,
  `over_budget`, `unsupported`, `requires_cloud`) and is `TransportError::Denied("inferd refused: <slug>")`, since
  the return type has no room for an `InferRefusal`. Tests: `porter-client/tests/prepare.rs` (4).
- **`SttBackend`** (`inferd::speech`): the closed set of engines a `Transcribe` turn runs on; one arm,
  `SpeechHost(SpeechHostClient)`, over the engine's Unix socket. One connection per utterance; dropping the turn drops
  the future, which sends `Cancel` and closes the socket, so inferd keeps no audio. `SpeechRunner::for_model` (a model
  whose engine kind is the speech host) and `transcribe` map the session's frames to `AudioChunk`s and the host's
  events to `Heard` deltas (a language the wire cannot spell is dropped); `SttShape` is the model's name and its
  `--chunk-ms` (560 when the entry says none). `runner::Turns` runs a `Transcribe` turn through it: the session machine's
  checked frames go to the turn through a channel, `end_audio` closes it. `Ears` is the `pipeline::Transcriber` a `Hear`
  stage runs on (it asks for the engine first).
- **The engine is supervised as a CPU engine from its catalogue entry**: `local::build` already made the unit
  (`stoker::command`: `paths.speech_host`, `--model-dir <snapshot>`, then the entry's args with `{socket}`); this lane adds
  `[engines] speech_host_libs`, which becomes the unit's `LD_LIBRARY_PATH` and a read-only bind of its sandbox, and a
  readiness probe that asks the host `Hello` (`HealthProbe::speech_hosts`; the HTTP `/health` probe cannot reach it).
  The socket is `$XDG_RUNTIME_DIR/inferd/speech_host-<model id>.sock` (the existing `<kind>-<id>` naming). A speech
  need that asks for `Stt` is listed like any other (`Engines::listed_for`); one that asks for `Tts` stays `Unavailable`.
- **Default grant for `org.quire.Voice`**: `dist/inferd.toml` `[callers.apps]` names `voiced.service` (docket's unit)
  as `org.quire.Voice`. A model on this computer is granted by its locality, so that row is the grant; the default floor
  keeps `voice` on this computer.
- **A session is `Waiting` while its engine loads, and a `Waiting` session refuses audio** (`Finished(Refused(Unsupported))`).
  voiced buffers until it sees the turn start (`utt.ready`); any other client must wait for `Routed` (or call `prepare`
  first). Not changed: the session machine is shared with every other turn kind.
- Tests (no real binary, model or library; a fake host on a Unix socket, the supervisor running it through a fake
  engine host on a private bus): `speech::stt::tests` (7), `local::tests` (speech unit), `config::tests`, `hosts::tests`
  (Hello probe; a child gets its unit's environment), `tests/speech.rs` (6: streaming partials and a final, the audit entry,
  restart after a crash with a turn cut mid-utterance, `Prepare`, no host, Hear then Answer over a text model).
- Not built: the text-to-speech runner (above);
  the systemd transient-unit host (the sandbox spec is built, the host is a child process).

### Owner: to run it for real

1. Build the host (stoker `dev/build-sherpa.sh` once, then `cargo build --release` in `speech-host-sherpa`) and the
   Nemotron weights (stoker FINDINGS "Fill V-H": `csukuangfj2/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11`
   at `ab43d895f5985b1bbab8b6eac8607fcdc05343f3`, into the Hugging Face cache so `models--csukuangfj2--...` exists).
2. `$XDG_CONFIG_HOME/quire/inferd.toml`: `[engines] speech_host = "<path>/speech-host"`,
   `speech_host_libs = "<sherpa install>/lib"` (the directory with `libsherpa-onnx-c-api.so` and `libonnxruntime.so`),
   `hf_cache` if it is not the default; the stoker catalogue directory must hold `nemotron-3.5-asr-streaming.toml`.
3. `[callers.apps] "org.quire.Voice" = ["voiced.service"]` (in `dist/inferd.toml`).
4. Start inferd, then voiced; `Voice1.Prepare` starts the engine (about 1 to 2 s to load), the first hold hears.

## Lane sheet-asks (sill's three accounts-sheet asks, 2026-10-07)

- **`ConsentAnswer::AddAccount`** (wire `{"kind":"add_account"}`, in a sheet input `{"kind":"answer","v":{"kind":"add_account"}}`).
  On this answer `AccountService::choose` runs the add sheet (`ProviderHint::Any`, the same window) with the alert's ask
  attached (`add_and_allow_for`): one grant, the add's own `Granted` audit line, no second prompt; the reply is `Chosen`
  for the new account. A cancelled or failed add answers as the add did (`Dismissed`, ...) and stores no grant; a new account
  that does not meet the need is kept, ungranted, and the reply is `NoFittingAccount`. `settle` treats the answer as dismissed
  (the service runs the add before it settles). Tests: `porter-families` `acceptance` (`add_account_on_the_alert_adds_and_allows_in_one_step_with_one_grant`,
  `cancelling_the_add_from_the_alert_grants_nothing`), `accountd` `bus_sheets` (`add_account_on_the_alert_opens_the_add_sheet_and_cancelling_it_grants_nothing`).
  Open: the bus test cannot finish an add (the fake provider is `Done` at once, which the machine does not store), so the
  success path is proven over `TestSheets` and a real Nextcloud fake, not over the bus.
- **`ProviderRow.kind: RowKind { Provider, Generic }`** (serde `default`, slugs `provider`, `generic`). Rows are built by
  `ProviderSpec::sheet_row` (porter-provider), the one place `AccountService::drive` takes them from the installed providers;
  a file whose id begins `generic-` (generic-imap, generic-dav, generic-jmap) is `Generic`. Test: `porter-provider`
  `shipped_files` `the_generic_files_make_generic_rows_and_every_other_file_a_provider_row`; core round trips.
- **`SheetInput::OpenAgain`** (wire `{"kind":"open_again"}`). On `Stage::Browser` the machine answers `SheetEffect::OpenBrowser(url)`
  (new effect; no state change, nothing cancelled or fed); off the browser step it is dropped. There was no open effect before:
  the host opened a page it was shown. `drive` serves `OpenBrowser` by showing the `BrowserWait` view again over the same
  link, so a host that opens the page on seeing the view (accountd's terminal host, sill) opens it again through its own
  portal path. Tests: `open_again_on_the_browser_page_opens_it_again_and_does_not_restart`,
  `open_again_means_nothing_off_the_browser_page` (core), `open_again_shows_the_browser_page_again_without_restarting_the_sign_in` (porter-service).
- `porter-fake`: `Scripted::AddAccount` (additive). Sill's fallbacks for the three asks end when sill switches to these.

## Lane manual-server (the hand-typed server form, 2026-10-07)

- **The form.** When the lookup finds no server the generic mail sign-in asks `porter_core::sheet::manual_form`: `Protocol`
  (a choice: `imap`, `pop3`, `jmap`), then for IMAP and POP3 `Server`, `Security` (`tls`, `starttls`), `Port`, `OutgoingServer`,
  `OutgoingSecurity`, `OutgoingPort`, and for JMAP `SessionUrl` and `Token` (a secret, optional: an API token instead of the
  password typed on the first form), then `Username` (optional; empty is the address). Hosts draw every field from `FieldKind`;
  `FieldKind::choices()` lists the values of a choice field (a pop-up or segmented control sends one of those texts). Ports,
  host guesses (`imap.`/`pop.`/`smtp.<domain>`, `https://<domain>/.well-known/jmap`) and the security prefill come from the
  form; an empty port is the usual one for protocol and security (993/995/143/110, 465/587).
- **Shape: new `FieldKind` variants** (`Protocol`, `Port`, `Security`, `OutgoingServer`, `OutgoingPort`, `OutgoingSecurity`,
  `SessionUrl`) plus `FieldKind::choices()`; `FieldSpec` is unchanged and `SignInView` JSON written before still reads
  (test `an_old_sign_in_view_still_reads`). `ProblemKind::Invalid` is new (a field-level problem: port not 1-65535, host with a
  scheme or path, a session URL that is not `https`, plain off this computer). Consumers match `FieldKind` and `ProblemKind`
  exhaustively (sill `accounts_sheet/props.rs`, mailo E5 `map.rs`): see the lane report for the exact diffs.
- **Validation lives in porter-core** (`sheet/manual.rs`: `refit`, `form_problem`, `parse_manual`); the stage machine refits the
  form to the protocol typed and returns the first bad field as `SignInView.problem` before anything is sent. A host keeps
  Continue off while `form_problem(fields, answers)` is `Some`. `plain` security is accepted for a loopback host only (the
  one place `ServiceEndpoint::check` allows it); `SECURITY_CHOICES` never lists it and porter-discover never offers it.
- **POP3 is a relayed protocol**: `UrlScheme::{Pop3, Pop3s}` (110/995), `EndpointProtocol::Pop3`, `Family::Pop3` relays
  and serves Mail, `porter-proxy` `Pop3Relay` (`CAPA`, `STLS`, `USER`/`PASS`, `AUTH PLAIN`, `AUTH XOAUTH2`; the app is greeted
  `+OK porter relay ready` in the transaction state, and its own `USER`/`PASS`/`APOP` are answered `+OK` and never forwarded,
  `AUTH`/`STLS` refused). `FakePop3` in porter-fake-servers. A POP3 account's mail claim names `MailTransport::Pop3`.
  Tests: `porter-proxy` `pop3::tests` and `tests/pop3.rs`, `accountd` `tests/relay.rs`, `porter-families` `tests/generic.rs`.
- **JMAP API token**: `Credential::Bearer(SecretText)` (serde `bearer`; every older stored credential still reads). It is filed
  under `SecretPurpose::Password` (an account has one secret; re-signing in with a password replaces a token and the other
  way round), the generic sign-in fetches the session with `Authorization: Bearer`, and `plan_relay` presents a `Bearer`
  as `RelayAuth::AccessToken`.
- **Open: no sign-in probe for IMAP, POP3 or SMTP.** `porter-families` has no socket seam, so a typed server is not logged in to
  at add time; a wrong password or port is found by the first relay (`NeedsReauth` or `Offline`). JMAP is checked (the session
  fetch is the login). Closes with a socket seam in `Io` and a probe per protocol, or the decision that the relay's first
  refusal is the check (same row as Generic IMAP above).
- **Open: a looked-up POP3 server is still not an endpoint.** `porter-discover` reports POP3 as data (`Found.pop3`); only the
  typed form builds POP3 endpoints. Mapping `Found.pop3` to `Family::Pop3` endpoints is the caller's (mailo E3 follow-up).
- **Open: the form asks the JMAP token beside a required password.** The first form asks the password; a person with only a
  token types any text there and the token in the `Token` field (the token is what is stored). Closes when the first form
  learns the protocol (an `Address`-only first form for a typed JMAP server).

## Standing facts

- No ds-core: a closed set's serde form is its slug; UI crates map slugs to labels.
- `todo!()` is allowed only behind a frozen interface; every such stub is listed above.
- deny.toml is quire's verbatim (the unused MPL allowance warns).
- The provider file is `ProviderSpec`'s serde form; every field is written, none defaulted.
- porter-core's vocabulary is `VocabVersion(3)`: endpoints on `Account` and `Candidate`, `OpenAuthenticated` and the refusal `EndpointNotGranted` joined (2 added computer use, `DataClass::Voice` and `GrantKey.space`). Version 3 is the first a file is written with (`FIRST_PERSISTED`), so a later bump needs a migration row.
- `AuditEntry.class` (the request's data class) has no serde default: DataClass has none, and nothing reads old `audit.jsonl` lines back, so old lines do not parse.
- inferd's replay engine takes `record = "<file>"` (prompts to disk, 0600, replay engines only, never in dist). inferd's `test-proc-root` feature (off by default, never in dist) honours `INFERD_PROC_ROOT=<dir>` for caller lookup (`<dir>/<pid>/cgroup`); without it the variable is ignored with a line on stderr.
- `StageNote.name` (the model's label) is read by the agent session's footer ("Answered by <name>"): inferd fills it from stoker's `ModelEntry.label` of the model that ran the stage, local or hosted, so the label lives on inferd's `Routing` and the session `Phase`, not on `ServedBy` or `ModelCard`. A model with no catalogue entry (the test `with_remote` cards) has no name. The live session sends the `Answer` note once per chat or task turn (a second turn on the same session sends it again); a voice chat turn runs `run_pipeline`, which sends one note per stage, and the session's own note is replaced for that turn (the machine sends neither it nor `Routed`): exactly one `Answer` note per turn.

## Voice chat on a live session (pipeline-wire)

- **Wire form (there was none: `ChatRequest` has no audio part)**: on a language session the client sends a `Transcribe` request (how the audio is heard: rate, language), `Audio` frames, `EndOfAudio`, then the `Chat`. The session machine keeps the audio (`Phase::Hearing`, `HeardAudio`, at most `MAX_HEARD_MS` = 60 s, else `Failed(Unreadable)`; no frames is also `Unreadable`; v1 rate only) and the chat starts a pipeline turn (`SessionOut::Hear` then `StartTurn`; `TurnRunner::start_heard`). A speech session's `Transcribe` is unchanged.
- **The `Waiting` rule is unchanged** (audio while `Waiting` is refused) except for this: a `Transcribe` that arrives while a language session is `Waiting` opens `Hearing` with the engine `Loading`, so audio is buffered (bounded as above) and the chat waits for `EngineReady`. A client cannot see readiness before the first turn (`Routed` goes out with it), so buffering is what makes "send everything at once" work.
- **Planning**: `inferd::pipeline::Hearing` plans per turn with `plan` (inputs text and audio, answer text, the app's offered hosted models, `settings.describe_images`), so the class set includes `voice`. The Answer stage runs on the model the plan chose, which can differ from the one the session was routed to (a stricter floor); it is asked for (`want`) before its turn. The audit entry still names the session's routed model; the reply names the one that answered.
- **Stage events** for a voice chat: `[Why]`, `Routed`, `Stage(Hear, name)`, `Heard`..., `Routed`, `Stage(Answer, name)`, text, `Finished`. `Routed` is sent per stage (twice in a two-stage turn), not once per session, on a voice-chat turn.
- **Hear failure** ends the turn `Failed(..)` (`Unreachable`, as a crash in flight does), not `Refused`; no speech model at all is `Refused(Unavailable)` from the planner. The session stays usable.
- **Gap**: a text model that takes audio runs one stage with no `Hear`, but `ChatRequest` has no audio part, so the audio is not delivered to it; no catalogue entry sets `LlmFeature::AudioIn` yet (the test rig adds it to a card). Closes with an audio part on the chat and the entry field.

## Lane scenario-fixes (what the jailed accounts scenarios found, 2026-10-07)

- Once grants (design/31 §5.4 decision): a `GrantScope::Once` grant is spent by its first use of any kind, a token issued or a relay opened (`OpenAuthenticated`, `OpenLinked`). §5.4 says "the first token issued", written before relays existed. The relay spends it once its plan is made (before the dial), so a second open is `UnknownGrant` and the app asks again through `Find`/`Choose`; a dial that then fails does not give the use back. `Peer.ResolveKey` still does not spend one (the row under "Stubs" says so). The `GrantScope::Once` doc in porter-core says the same. Test: `an_allow_once_grant_is_spent_by_the_relay_it_opens_and_the_second_open_is_refused`.
- A refused refresh (the provider answers `Unauthorized`: `invalid_grant`, a revoked token) sets the account `NeedsReauth` wherever it is met: `IssueToken`, `OpenAuthenticated` (token accounts), `rediscover`. `AccountService::refused_refresh` does it (state and one store write); accountd publishes after every relay call, so the `NeedsReauth` signal, the shell's `State` change and the settings `Changed("accounts.<id>.state", "needs_reauth")` follow once; a second refusal finds the state already set and says nothing more. A missing or unreadable secret is still only the app's `NeedsReauth` refusal, with no state change (nothing was refused by an issuer). Tests: `a_refused_refresh_on_issue_token_...`, `a_refused_refresh_on_open_authenticated_...` (`FakeProvider::refusing_refresh`).
- "Sign in again" with no grant: `Account.Reauthenticate` by the `SheetHost` or `Settings` role, and Settings' `accounts.<id>.reauth` action, go through `AccountService::reauthenticate_any` (a `Host` method with a default of `Unavailable`); an app still needs a grant it holds (it does not even see the account object). The `Settings` role may call `Account.Reauthenticate` too; it still reads no property of `Account` (`sees` is unchanged).
- Local runtimes in the real accountd: `main` keeps `ollama`, `llama-cpp` and `lm-studio` (the `AuthKind::LocalRuntime` providers no family serves) in the service's catalogue (`AccountService::with_local_runtimes`, `providers::local_runtimes`) and no longer prints "no family serves provider" for them. They are not offered by the add sheet (no `Provider`). Test through the binary: `the_binary_makes_accounts_of_the_local_runtimes_inferd_reports` (a `test-proc-root` build).
- Settings module and a changing key set: `SettingsModule1` has no signal for "the key set changed" (`Describe`, `Get`, `Set` and `Changed(key, value)` only), and detent's `Followed::Changed(..)` reads the schema again on ANY `Changed` (accounts pane: `AccountsInput::Changed` is `Reload`). So no interface change: accountd sends `Changed` for a row of the account that came or went: `accounts.<id>.state` with the state slug when it appears, with `removed` when it goes (its rows are gone from the schema by then), and `accounts.<id>.label` with the label when it changes (`hub::settings_news`). Not announced yet: a new or revoked grant row and a toggled service row (a `Set` already sends its own `Changed`; a grant made by an app's `Choose` is not), so detent's open pane shows a new grant on its next reload.
- syncd's app id is `org.quire.Sync` everywhere (the shipped caller table row, tests, storage docs, this file); the spelling with a trailing `d` was a stray, and `syncd/tests/dist.rs` fails if it returns. The bus name is `org.quire.Sync1`, a different thing.
- inferd.toml's `[engines]`, `[probe]` and `[callers]` tables are warned about by detent ("unknown key"), because detent's `unknown_keys` (detent-files) counts every leaf no `[[key]]` of `dist/inferd.settings.toml` names, and quire's `Schema` (`ds-settings` `schema/program.rs`: `app`, `file`, `version`, `key`) has no way to declare a table that is not a setting. Ask for quire and detent, not done here (see the report): `Schema` gains `foreign: Vec<String>` (tables the program owns that are not rows), `dist/inferd.settings.toml` says `foreign = ["engines", "probe", "callers"]`, and `unknown_keys` skips a leaf under one.
- porter-rig gaps closed: `FakeGraph` answers `GET /v1.0/me` (`mail` and `userPrincipalName`, `GraphHandle::set_mail`, the rig sets the mail fakes' user) behind the same bearer check; `porter-rig-client add-account [provider]` and `reauthenticate <account>` (`{"result":"added"|"reauthenticated"|"refused","refusal":..}`); `GET /imap/attempts`, `/smtp/attempts`, `/pop3/attempts` on the control endpoint (user and outcome only); `porter-rig-servers --token-lifetime-s N` (`IssuerHandle::set_token_lifetime`); syncd reads `SYNCD_RESCAN_S` (whole seconds, at least 1) as the supervisors' rescan in a `test-proc-root` build only (both the storage and the PIM supervisor; the default stays 600 s in every build). A client run in a jail still needs a sheet host to finish `add-account`: against no host it answers `refused`/`Unavailable`.

## Lane reauth-grant (sign in again ends at Done; Settings lets syncd sync an account, 2026-10-07)

- Signing in again never shows the "Choose what to use" review, for every family. Microsoft's sign-in (`MicrosoftSignIn::conclude`) answers `Done` straight after its Graph read when started in `SignInMode::Reauthenticate` (it still reads `/me`, so `store_renewed` can refuse another person's login); the generic, Nextcloud and API-key families already did. As a net for any family, the sheet machine (`porter_core::sheet::step`, `Progress::Review` while `Purpose::Reauthenticate`) confirms a review at once with no choices instead of drawing it. `store_renewed` replaces only credentials and state, so the account's capabilities, endpoints and service toggles are unchanged. Tests: `signin::a_loopback_sign_in_again_ends_done_without_a_review`, `a_device_code_sign_in_again_ends_done_without_a_review`, `a_sign_in_again_that_offers_a_review_is_never_asked_again_and_keeps_its_services`, `machine_tests::a_review_offered_to_a_sign_in_again_is_confirmed_unchanged_not_shown`.
- Settings can let syncd sync an account: accountd's settings module has `accounts.<id>.sync.files` ("Keep this account's files on this computer") and `accounts.<id>.sync.photos` ("Back up photos"), toggles that default `off`, Settings role only, never agent-settable. `on` records an always-allow `Storage` grant of class `Files` or `Photos`, usage `Background`, for `org.quire.Sync` (an unsandboxed native app, as the caller table names it), replacing a kept refusal of the same key; `off` removes it (audited `Granted` / `Revoked`). The dataset drops at syncd's next rescan and the mirror's files stay (W6c's rule). A row is offered for an account only when syncd can mirror it: Storage present and a Graph endpoint (the only store syncd mirrors; a WebDAV account has no row), and Photos only when the store takes resumable uploads (`chunked_upload`), which original photos need (`porter_service::sync_offers`). `Changed` is sent by the module's `Set` and, when syncd's grant appears or goes by any other route (the grant's own Revoke row), by the hub (`settings_news`). The Photos datasets still run only behind syncd's Photos switch (off by default: there is no Photos app).
- Ask for detent (not done here, detent is read only): `detent-model/src/accounts/model.rs` `classify` keeps a `KeyKind::Toggle` row only when its path holds `.service.`, so the two sync rows are dropped and the pane shows nothing new. Wanted: a `Row::Sync` for a toggle whose path ends `.sync.files` or `.sync.photos`, an `AccountSync { path, label: spec.label, on }` list on `Account` (the label is the plain sentence the module sends, not `humanize` of the kind), and `detent-ui` `detail_view` drawing each as a switch under the account's services, set like a service switch (`Set(path, "on" | "off")`).
| I5 git deps for stoker and quire | Done in the root `Cargo.toml`: stoker `https://github.com/PoHsuanLai/stoker` rev `b24f6d456e7915f1052c722f5aebabf1ee0f0e74`, quire `https://github.com/PoHsuanLai/quire.git` rev `db321897069ad54de99b6cab95e16ffa51b4c618`. Local override: a git-ignored `.cargo/config.toml` with `[patch."<url>"]` lines to `../stoker/crates/<crate>` and `../quire/crates/<ds-crate>` (ARCHITECTURE section 1). Open: each sibling-path consumer needs the same `[patch]` in its own workspace root (committed there), or its graph holds two copies of ds-* and stoker crates; the owners of sill, detent, casement, cua, docket and almanac apply it (diffs in ~/rs-wt/i5-gitdeps/consumer-patches) |

## Lane grant-names (a grant row names the app, not its bus id, 2026-10-07)

- Settings' Apps group: a grant's row is a sentence, "Sync can use Files" (Allow) or "Mail can't use Contacts" (Deny), with the Services group's own label for the service (`settings_keys::service_label`, also the Service switch's label: an exhaustive match over `CapabilityKind` with no wildcard arm, giving the words of sill's account sheet, `accounts_sheet/words.rs` `service_name`: Storage is "Files", Llm "Language model", ImageGen "Image generation", KeyValue "Small synced items"; a new kind must be named there and in sill's copy, nothing checks that the two agree). The app is named by, in order: the caller table's `name = "..."` on a `[[caller]]` row (`porter_dbus::CallerRow.name: Option<AppTitle>`, `CallerTable::title_of`; `dist/callers.toml` names Settings, Shell, Intelligence and Sync), the unlocalised `Name=` of `[Desktop Entry]` in `<app id>.desktop` under `$XDG_DATA_HOME/applications` then each absolute `$XDG_DATA_DIRS` entry's (default `/usr/local/share:/usr/share`; `accountd::AppNames`, paths in `Paths::applications`), then for an app `org.quire.Agent.<program>` the `label` of the loaded provider file that runs `<program>` with `auth.kind = "agent_login"` (`AppNames::with_agents`, built once at start from the loaded specs: `claude-code.toml` gives "Claude Code"; anthropic.toml also names the program but is a key provider and does not name the app), else the app id. Desktop entries are read when the schema is built (Settings' `Describe`), not cached: a few small files per granted app, so an installed or renamed app shows on the next read. The row key (`accounts.<id>.grant.<grant>`) is unchanged.
- Open: a localised `Name[xx]=` of a desktop entry is not read (the person's locale is not known to accountd); the unlocalised name shows. Closes with a locale-aware `AppNames` if Settings is ever localised.

## Lane i1-filekeys (a file-backed key store for private-bus and scratch runs, docket ask I1)

- **I1 file-backed keys (`test-keys`)**: porter-secrets' `FileSecrets` (feature `test-keys`, off by default, `cfg(unix)`) is one 0600 JSON file (`{"version":1,"items":[{"service":"porter","account","purpose","credential"}]}`: the keyring backends' addressing, plain text by design). It is created 0600 with its directories and refused (`FileSecretsError::Mode`, `NotAFile`, `Corrupt`, `NotAbsolute`, `Io`) on open and on every use when a group or other bit is set. Every use takes an advisory lock on `<file>.lock` (a daemon and `accountd add`, two processes, are serialised) and a change is written to `<file>.tmp` and renamed over the file (a crash leaves the original whole and a stale temp, replaced by the next write). accountd's `test-keys` feature turns it on: `ACCOUNTD_KEYS=file:<absolute path>` picks it for the daemon and for `accountd add`; unset keeps `Oo7Secrets`; in a test build any other value (not `file:<path>`, a relative path, a loose-mode or unparsable file) stops the daemon before it serves (memoryd's choice: it refuses an unreadable value too); a shipped build ignores the variable and logs one line, without the value, saying so. Both say which store they chose on standard error (never a value). Not default and not in dist: `accountd/tests/dist.rs` (feature declared, not default, the normal dependency does not name it, no `dist/` file names the knob) and `scripts/check-boundary.sh` (`cargo tree` of accountd and porter-secrets shows no `test-keys`). Covered: `porter-secrets/tests/file_store.rs` (round trip, 0600, mode refusal on open and later, corrupt files, crash between temp and rename, concurrent writers across threads and stores), the shared contract suite over `FileSecrets`, `accountd::keysel` unit tables, `accountd/tests/file_keys.rs` (the real binary on a private bus with no Secret Service: `accountd add openrouter` into the file, the daemon over the same file, a porter daemon's `ResolveKey` returns the key on a sealed memfd, a bus monitor sees no message carrying it; the refusals; the shipped-build twin). Not covered: Windows and macOS (the store is `cfg(unix)`), a group-writable key directory (only the file's mode is checked), and a crash between the rename and the directory's own sync (the rename is not fsynced into the directory). A scratch accountd finds provider files in `/usr/share/porter/providers`, `$XDG_DATA_HOME/porter/providers` and each `--providers DIR` (ARCHITECTURE section 6, "Run a scratch accountd", has the recipe).


## Lane p1-agent-provider (accounts for external coding agents, agent-session ask P1)

Covered (acp-sessions.md section 8 P1, P5's `NeedsLogin`, P8's role; section 12 R3; design/31 C5, C6, R7, R8):

- **`AuthKind::AgentLogin`** (`agent_login`): an agent program that signs itself in. The account holds NO credential: `secret_purpose` is `None`, the add flow files nothing (`AgentLoginSignIn` ends `Done` with `credentials` empty), a session mints no token (`access_token` is `Forbidden`), revoking is `Unsupported` (there is nothing at a provider). Its state is the agent's word and nothing else.
- **`AccountState::NeedsLogin`** (`needs_login`) and **`AgentState { Ready, NeedsLogin }`** (what the launcher reports; `account_state()` maps `Ready` to `Ok`). `AccountState::needs_person()` is `NeedsReauth | NeedsLogin`: the shell hears one signal for both (`hub::events` sends `NeedsReauth` when an account first needs a person, and `Manager.NeedingReauth` lists both), so sill needs no new signal. A new `agent_login` account starts `NeedsLogin` (`add_store::first_state`); "sign in again" on one (`store_renewed`) leaves the state as the agent last said it.
- **`Capability::Agent(AgentCap)`**, kind `agent`, family `Family::AcpAgent` (slug `acp_agent`, porter's snake_case style; the brief wrote `acp-agent`), not an AI kind (inferd never routes it, no `[ai]` table). `AgentCap { program: AgentProgram, key_env: Option<EnvName>, base_url_env: Option<EnvName>, protocols: BTreeSet<AgentProtocol> }`. `AgentProgram` (lowercase letters, digits, `-`, at most 48) and `EnvName` (uppercase letters, digits, `_`) are validating newtypes; `AgentProtocol` is `OpenAiCompatible` (`openai_compatible`), `AnthropicMessages`, and `GenerateContent` (added beyond the brief's two because Gemini CLI speaks the Gemini API, and declaring it OpenAI-compatible would be a false fact). `Need::Agent(AgentNeed { program, protocols, base_url: Offered })`; `matches` checks the program (`Shortfall::Program`), then the protocols (`Protocols`), then a base-URL override if asked (`BaseUrl`).
- **`Subject::Agent(AgentProgram)`**: `effective()` keeps one claim per (subject, kind), so an account that may run several programs needs a subject per program; this is it. An agent file's and a key file's agent rows become `Provenance::Declared` claims with this subject (`AgentLoginProvider::discover`, `key::claims`).
- **Vocabulary 5** (`VocabVersion::CURRENT`): new variants only (`CapabilityKind::Agent`, `Capability::Agent`, `Need::Agent`, `Subject::Agent`, `AuthKind::AgentLogin`, `AccountState::NeedsLogin`, `Family::AcpAgent`), no stored field changed. `MIGRATIONS` gains the 4 to 5 step (identity) and `a_document_stored_at_vocabulary_four_is_read_unchanged` is its fixture; `an_agent_login_account_survives_its_document_and_keeps_its_slugs` freezes the new stored slugs.
- **Provider files**: `claude-code.toml`, `gemini-cli.toml`, `codex.toml`, `acp-agent.toml` (`[auth] kind = "agent_login"`, `[discovery] kind = "fixed"`, one `acp_agent` row each). `anthropic.toml`, `openai.toml` and `google-ai.toml` gain one agent row each (claude-code, codex, gemini-cli): that is the typed list of programs that may use the key (a row per program, the same `AgentCap` the agent's own file declares; a test checks the two copies agree). `openrouter.toml` and `moonshot.toml` name none (see unverified). `parse_provider` refuses (`ProviderFileError::AgentRows`) a row of kind `agent` not served by `acp_agent` (and the reverse), the same program twice, and an `agent_login` file with any other row.
- **accountd**: `Peer.SetAgentState(account s, state s)` (`ready` or `needs_login`; returns nothing), role `AgentLauncher` ONLY (`PorterDaemon` and every other role get `AccessDenied`). `CallerRole::AgentLauncher` is narrow: `Core::identify` refuses it `AccessDenied` for every other method (`Standing::Launching` is the only standing it has). It is never granted by default: `dist/callers.toml` has no row for it, and its comment shows the row a machine writes for docket-acp (`tests/dist.rs` reads that comment's row back). The call is `InvalidArgs` for an unknown account, an account that is not `AgentLogin`, or a word that is not an `AgentState`. A change is saved and published (state `PropertiesChanged`, Settings `Changed("accounts.<id>.state", ..)`, the shell's `NeedsReauth`); a repeat of the same word writes nothing. Settings shows an agent account with the others (state row, service switch, grants, Remove); its "Sign in again" is replaced by `accounts.<id>.sign_out` ("Sign out"), which only sets `NeedsLogin` (all porter holds). Porter never reads or touches the agent's own login files.
- Tests: porter-core `round_trip.rs` (`agent_values_keep_their_slugs_and_hold_no_secret`, the agent rows of every-capability/need), `matching/tests.rs` (six `agent` rows), `store/tests.rs` (two); porter-provider `parse/tests.rs`, `tests/shipped_files.rs` (`the_agent_files_hold_no_secret_and_name_their_program_and_variables`, `a_key_provider_names_the_agent_programs_that_may_use_its_key`); porter-families `tests/agent_login.rs`, `tests/api_key.rs` (`a_key_account_holds_a_claim_for_each_agent_program_its_file_names`); accountd `tests/agent_login.rs` (SetAgentState accepted for `AgentLauncher`, refused for every other role and a stranger, the launcher refused everything else, invalid arguments, the settings schema and sign-out and `Changed`, the shell told, the add flow storing no credential, which account runs which program and a key account refused for the others) and `tests/dist.rs`.

Unverified product facts (no network; none of these is vendored or in the spec, so each is from memory and marked so in the file's comment). Check against each product's own documentation before P2/P4 rely on them:

- Claude Code: `ANTHROPIC_API_KEY` as the key variable and `ANTHROPIC_BASE_URL` as the endpoint override; that it speaks Anthropic Messages to its model. (The spec itself says "believed supported ... verify per agent".) The agent session adds (from memory, high confidence): `ANTHROPIC_AUTH_TOKEN` is the bearer-token alternative to the key variable; `AgentCap` declares one key variable, so P4 sets `ANTHROPIC_API_KEY` only.
- Codex: `OPENAI_API_KEY` and `OPENAI_BASE_URL`; OpenAI-compatible protocols (`AgentProtocol::OpenAiCompatible` covers both wire shapes and does not say which). The agent session adds (medium confidence): Codex speaks the Responses API by default; a custom provider entry in `~/.codex/config.toml` takes `wire_api = "chat" | "responses"`, so a Chat Completions endpoint works through such an entry, and a plain `OPENAI_BASE_URL` override exists in recent versions. For P4 (inferd as the agent's endpoint): either inferd serves Responses, or the launcher writes a provider entry with `wire_api = "chat"` in the agent's own HOME; the second is the smaller first step.
- Gemini CLI: `GEMINI_API_KEY` as the key variable (`GOOGLE_API_KEY` with Vertex); NO base-URL variable is declared because none is known (so `Need` with `base_url = Present` falls short, `Shortfall::BaseUrl`); protocol Gemini generateContent. The agent session adds (low confidence): the genai SDK it uses may honour `GOOGLE_GEMINI_BASE_URL`, but whether the CLI passes it through is unknown, so `base_url_env` stays unset.
- Antigravity's `agy` CLI (the owner's second test agent, spec R7) has no ACP mode, only a headless stream-json mode; it may need its own provider file later. Not in P1.
- Generic `acp-agent`: nothing is known of it, so no variable is declared and `openai_compatible` is a placeholder; a person's own provider file can name a particular program with its real variables.
- OpenRouter as an Anthropic- or OpenAI-compatible endpoint for these agents (a key account usable by `claude-code` or `codex` through OpenRouter's compatibility layers) is not declared: `openrouter.toml` names no program. Closes when the per-product facts are checked.
- Whether an agent reports "signed in" in a way the launcher can observe without reading the agent's token store (ACP `authenticate` success, a session starting): spec section 11 lists the exact JSON of `authMethods` as unverified. The launcher side of that is docket's (S4).

What the next lanes add:

- **P4 (metering proxy)**: `AgentCap::base_url()` is the question "can this program be pointed at a loopback endpoint"; P4's route for a key account is `Need::Agent { base_url: Present }` then a loopback endpoint with a dummy key. It needs no change here beyond filling the right variable.
- **P5**: done in lane p5-audit (`KeyResolved`, `AgentStateSet`, `AgentLoginAsked`/`AgentLogoutAsked`; `ProcessCredentialIssued/Revoked` written since lane p2-handoff). `NeedsLogin` itself is done.
- **P2**: done in lane p2-handoff (`Tokens.IssueProcessCredential` / `RevokeProcessCredential`, gated to `AgentLauncher`; see "Lane p2-handoff"). The method takes `Always` and a `Session` grant (the per-session scope, closed by lane session-scope: see "Lane session-scope") and refuses `Once`.
- **Marks**: the agent files reuse their vendor's existing glyph names (`anthropic`, `gemini`, `openai`); only the generic `acp-agent` uses a new one, `agent` (sill to draw, or to fall back as it does for an unknown mark).
- **sill**: `sill-model` `AccountStanding` parses `needs_reauth` only; an agent account in `needs_login` reaches the shell as the `NeedsReauth` signal, and the shell's "Sign in again" button then calls `Reauthenticate`, which for an agent account runs the (empty) review and leaves the state as the agent said. The wording ("Sign in inside the agent") and the button belong to sill; until then the notice is the generic one.
- **detent**: `classify` shows the service switch of kind `agent` as "Agent" and does not know the new `sign_out` action; it is shown as an unrecognised action row at worst.

## Lane p4-agent-endpoint (inferd as an external agent's model endpoint, agent-session ask P4)

Covered (acp-sessions.md section 8 P4, P6, P8; section 12 R3; the agent session's additions of 2026-10-08):

- **Bus**: a second interface, `org.quire.Inference1.Agents`, on inferd's bus name and object (`/org/quire/Inference1`; `Inference1`'s seven members are unchanged and `tests/introspection.rs` still counts them). `dbus/org.quire.Inference1.Agents.xml`, `Bus::InferenceAgents`, `AgentsProxy`/`AgentsSkeleton`. `OpenEndpoint(program s, route (sssas), class s, protocols as, options a{sv}) -> (sssqsss)` and `CloseEndpoint(session s)`. The route is `(kind, id, models, ids)`: kind `account` (id an `AccountId`) or `model` (id a catalogue id: a supervised engine, a runtime the person runs, an attached engine); models `listed` (exactly the ids given; for a model route, none given means its own catalogue id) or `any` (a typed route option: whatever the agent names). The reply is `(session, scheme, host, port, base_url, token, protocol)`: scheme `http`, host `127.0.0.1`, an ephemeral port (typed fields, so the launcher can hand `host:port` to a sandbox allowlist), `protocol` the one `base_url` is for (`anthropic_messages`: the origin, for `ANTHROPIC_BASE_URL`, whose SDK appends `/v1/messages`; `openai_compatible`: the origin plus `/v1`), chosen as the first of Anthropic, OpenAI that the program lists and the route can speak. The unix-socket variant is dropped (agents take http base URLs).
- **Who**: `Role::AgentLauncher` (`inferd.toml` `[callers] agent_launcher = [units]`, never listed by default; the commented line is read back by `tests/dist.rs`; mapped from `CallerRole::AgentLauncher`, placed after cuad's row and before any app's so an app entry cannot claim its unit). The launcher is refused every other member of inferd (`Open`, `Availability`, `Prepare`, `Usage`, `Rescan`: `AccessDenied`) and every other role, and a stranger, is refused `OpenEndpoint` `AccessDenied`. Typed causes on the bus are `org.quire.Inference1.Error.{EndpointOff, UnsupportedProtocol, NoSuchRoute, NotGranted, FloorRefused, NoSuchSession, Unavailable}` plus the bus's `AccessDenied` and `InvalidArgs`.
- **Setting**: `ai.agents.endpoint = "off" | "on"`, default **off** (R5), a toggle row of the Intelligence page (hands-off for agents like the other `ai.*` rows); off refuses `OpenEndpoint` `EndpointOff`. Read at each open, so it takes effect for new sessions only; turning it off does not end sessions that are open (the launcher closes them, or the policy checks below refuse their requests).
- **The token and the port**: 32 random bytes from the kernel, prefixed `sk-porter-agent-`, fresh per session, compared in constant time, `Debug` shows its length only, in no log line, audit line, error or file, and on the bus exactly once, in the reply to the launcher's open (the bus-scan test counts: one message holds it). It ends with the session. **Any process on this computer can connect to the port: the token is the whole guard**, and the listener is plain HTTP on loopback (no TLS). A request without the right token is refused 401 from its head, before its body is read. Sessions end at `CloseEndpoint` (the listener is closed and every request in flight is cancelled before the call returns) and when the launcher's bus connection leaves (a `NameOwnerChanged` watch per launcher connection).
- **The wire**: `POST /v1/messages` (Anthropic, streaming SSE and whole), `POST /v1/messages/count_tokens`, `POST /v1/chat/completions` (OpenAI, streaming with `[DONE]` and whole, `stream_options.include_usage` honoured), `GET /v1/models` and `GET /v1/models/{id}` (Anthropic's list shape when the request has `anthropic-version`, OpenAI's otherwise). Auth is `x-api-key` or `Authorization: Bearer`; `anthropic-version` and `anthropic-beta` are forwarded to an Anthropic account. A path without the `/v1` prefix is accepted. HTTP/1.1, one request per connection (`Connection: close`), bodies by `Content-Length` or chunked, up to 64 MiB. Errors are in the protocol's own shape (`fail.rs`): 401 authentication_error, 403 permission_error, 404 not_found_error / model_not_found, 400 invalid_request_error, 413, 429 rate_limit_error, 529 overloaded_error (503 for OpenAI), 502 api_error. A provider's error body is never relayed (it may echo the prompt); the agent is told the kind.
- **Account route** (`account.rs`): the request is forwarded as the agent wrote it to the account's provider (`Cloud::agent_client`, the provider's door: `anthropic` is now in `Doors::real` at `api.anthropic.com/v1`, used only here, since a model turn still speaks only the OpenAI wire), with the real key in the provider's header (`x-api-key` for Anthropic Messages, a bearer otherwise), fetched with `Peer.ResolveKey` for each request on a sealed descriptor and dropped with the client. Nothing is mapped, so tool use and results, thinking, images, server tools and `cache_control` pass exactly. The one change: an OpenAI stream gets `stream_options.include_usage` so it can be metered. The protocol must be the provider's (`Reach.wire`): the other is refused at the open (`UnsupportedProtocol`) and 400 per request. The model id must be one the provider's catalogue prices (`Reach.model`), or the request is 404 (an unpriced id cannot be metered); the catalogue is the shipped stoker one, so an account serves what stoker prices for it, nothing newer.
- **Grant (P6)**: the audience is the program, as an app named `org.quire.Agent.<program>` (isolation `unsandboxed`), asked of accountd with the existing `Peer.Verdicts` under the session's data class, so no field was added to `GrantKey`, `porter-core` or accountd: the person grants it with `accountd add <provider> --allow org.quire.Agent.claude-code [--class <class>]`. "Held by the launcher" is enforced by who may open the session (the launcher role) and is not recorded in the grant. The verdict is checked at the open (`NotGranted` on the bus) and again at every request (403 `permission_error`; no key is fetched and nothing is sent). **Not checked**: the key account's `Subject::Agent(program)` claim (which programs the key file names) is not visible in `Verdicts`; the grant is what decides. Policy is checked at the open (`FloorRefused`) and at each request (403): a cloud route needs `ai.local_only` off and the class's floor `anywhere`; another machine of the person's needs the floor or `ai.attached.my_network`. **The class** is the launcher's to name at the open (`class`), as an app names it in `Open`; there is no default on the wire (an unknown slug is `InvalidArgs`), and the strict choice (`prompt`) refuses every cloud route and a `my-network` engine under the default policy.
- **Spend**: every request counts against the account's and the program's `ai.spend.*` caps exactly as a cloud turn does: the check before it is the same `Ledger::verdict` with the same estimate (a thousand tokens each way at the reach's price), the usage is read from the provider's reply (Anthropic `message_start`/`message_delta`/`usage`, OpenAI `usage`), priced at the reach, and recorded in the same ledger; a missing figure counts the estimate, never zero. Over the cap: **429** `rate_limit_error` (OpenAI: code `spend_cap_reached`) with `x-should-retry: false` (Anthropic's SDKs read it), nothing sent to the provider and nothing counted. A rejected request (non-2xx) counts no tokens. A request the agent abandons mid-stream is counted at what was read.
- **Audit**: one `AuditEntry` per request (`inferd::audit` lines, the same destination as a session's): app `org.quire.Agent.<program>`, the account, the catalogue model, the locality, tokens, the class, bytes out for a cloud route, and the new **`cost: Option<MicroUsd>`** (a `porter-infer` field, `serde(default, skip_serializing_if)`, absent for a model on this computer); never content, key or token. The launcher is not in the line (the program is).
- **Model route** (`local.rs`): the request is read into `ChatRequest` (`anthropic/request.rs`, `openai/request.rs`; the text-to-`TurnRequest` step is `porter-bridge`'s, unchanged), run through `Turns` against the pinned local model (same engine start, readiness, attached look and release as a session), and written back. Mapped, both ways and table-tested: text, base64 images (PNG/JPEG, as the bridge takes), tool declarations, tool choice (auto, any/required, named, none), parallel or not, tool calls (`tool_use` / `tool_calls`) and results (`tool_result` blocks, `is_error`; `tool` messages, which follow each other into one user message so parallel results stay paired), system and developer prompts, stop strings, `max_tokens`, temperature/top_p/top_k, thinking budget / `reasoning_effort` to an effort, a JSON-schema `response_format`, thinking blocks both ways (an Anthropic `thinking` block's `signature` and `redacted_thinking` data ride in `ThoughtSeal`; replies carry reasoning as a `thinking` block or `reasoning_content`; a local engine gives no signature, so an empty one). Dropped: `cache_control`, `metadata`, `service_tier`, `store`, `user`. **Refused, typed and naming the feature (400)**: server tools, `document` and other block types, image URLs, `mcp_servers`, `output_format`, `n` over one, `json_object`, audio parts, legacy `functions`. A stream starts at its first answer text or call; reasoning before it is held, so **a reply that is only reasoning is `ModelError::OnlyThought { stop, thought_len }`** (ask I4: new variant of `porter-infer`'s `ModelError`, lengths only) told as 502 `api_error` / `only_reasoning` with the length, never the thought, whether or not the agent asked for a stream. The risky part is the tool-call template the engine applies; it is covered here against the fake engine's OpenAI wire, not a real model's template.
- **count_tokens** (Claude Code may call `POST /v1/messages/count_tokens`, unverified): **served as an estimate**, a quarter of the characters of system, messages and tools, the same on every route; the provider is not asked, so it costs nothing and is not counted. A client that needs the exact figure has none to get here.
- **Model ids**: an id the route does not serve is 404 in the protocol's shape (never a substitute); `GET /v1/models` lists what it serves (for an any-model account route, what the provider's catalogue prices).

Not covered, and what each agent needs from the launcher (all product facts unverified; see the p1 list above):

- **The Responses API is not served** (`/v1/responses` is 404). Codex speaks Responses by default; the launcher writes a provider entry with `wire_api = "chat"` in the agent's own HOME (`~/.codex/config.toml`, from memory: `model_provider = "porter"`, `[model_providers.porter]` with `base_url = <base_url, the /v1 one>`, `env_key = "<a variable that holds the token>"`, `wire_api = "chat"`) and sets that variable to the token. `OPENAI_BASE_URL` alone is not enough while Codex defaults to Responses. The route's protocol must be `openai_compatible`, so an OpenAI-wire account (OpenAI, OpenRouter, Moonshot) or a model route.
- **claude-code**: `ANTHROPIC_BASE_URL=<base_url>` and `ANTHROPIC_API_KEY=<token>` (the key variable `AgentCap` declares; `ANTHROPIC_AUTH_TOKEN` also carries it as a bearer, which the endpoint accepts), protocol `anthropic_messages`, so an Anthropic account or a model route. The model ids Claude Code asks for must be ones the route serves (an any-model account route, or a model route `any`, or a list that names them); a model route answers any served id with its own model.
- **gemini-cli**: declares no base-URL variable, so the endpoint cannot be given to it (generateContent is not served either); P2 or its own login.
- **agy**: not covered (no ACP; its model endpoint settings are unknown).
- **Not done**: TLS on the listener; keep-alive and HTTP/2; server tools, documents, URL images; the Responses API; the provider's own `count_tokens`; cache-control pass-through to a model route (it has no cache); `OnlyThought` for an account route (the provider's reply is not read beyond its usage); a cross-protocol account route (an Anthropic request to an OpenAI-wire account); turning `ai.agents.endpoint` off ending open sessions; recording the launcher in the audit line; a connection cap per session (a request that sends nothing holds its task for two minutes); a client that drops during a non-streaming model-route turn does not cancel it (the turn ends and its write fails).
- **Interface asks for the owners of consumers**: `ModelError` gained a variant: docket's `action-review/src/infer.rs` matches it exhaustively and needs `ModelError::OnlyThought { .. }` in an arm (its own `Heard::OnlyThought` already exists); `Bus` gained a variant (nothing outside porter matches it).

## Lane google-route (the Google family, ready for the owner's own client, W5c, 2026-10-08)

Built, not run against Google: nothing here has touched a real Google client or the network.

- **What is done.** `porter-families` feature `google`: `GoogleProvider` / `GoogleSignIn` / `GoogleSession` / `GoogleEnv`. Sign-in is the installed-app flow (PKCE S256, loopback redirect, `client_secret` from the clients row, `access_type=offline`, `prompt=consent`, `include_granted_scopes=true`), the address from userinfo, a small read per service (Calendar `calendarList`, People `contactGroups`, Tasks `lists`, Drive `files?spaces=appDataFolder`), Photos shown by its granted scope, Gmail only for `byo = true`. Tokens: one access token for all granted scopes, a bearer for the `google_*` family slugs and row endpoints, an XOAUTH2 string for `imap`/`smtp` (BYO only), through accountd's relay unchanged. Refresh does not rotate; revoke posts the refresh token to Google's revoke endpoint when an account is removed. A granted-scope answer (`TokenResponse.scope`, new, defaulted) turns a service the person unticked into `Absent { TurnedOff }`. `providers/google.toml` gains endpoints on its rows, a picker row, a mail row and `matching`; accountd serves it. `porter-oauth`: `ClientRegistry::traits` (`MailRights`, `AppReview`) reads `byo` and `testing` from the same clients files (`ClientEntry` is shared with mailo and is not changed). `porter-fake-servers`: `FakeGoogle`, `Google` (issuer in `Style::Google` + APIs), `IssuerHandle::{set_style, require_client_secret, authorize_queries, access_scope}`; `porter-rig-servers --google` (rig.json `google`, planted `GOOGLE_CLIENT_SECRET`).
- **No client row.** The provider is listed and served; `SignIn` answers `Failed(NeedsClientId)` before anything is opened or dialled. The wording ("Google sign-in needs a client id: see docs/google.md") belongs to the sheet (quire `ds-shell`, which words `NeedsClientId`): it says nothing of Google or the guide yet. An existing token path with no client is `Unreachable`, not `NeedsReauth`.
- **The seven days.** A client row with `testing = true` gives the account `Restriction { Unverified { 100 }, SevenDays, signed_in }` (closed by lane google-followups: the date, the reason and the Settings row are there; see that section). A refresh Google refuses is `ProviderError::Unauthorized` (so `NeedsReauth` in accountd); the session's own `reauth_reason()` (`ReauthReason::TestingExpiry`/`Revoked`, family-local) stays for the family's tests.
- **Unverified (no network; check against Google's pages when the owner's client first runs):** the endpoints in `Issuer::Google.endpoints()` (`accounts.google.com/o/oauth2/v2/auth`, `oauth2.googleapis.com/token`, `/revoke`); userinfo at `openidconnect.googleapis.com/v1/userinfo`; every API base and probe path in `google.toml` and `probe.rs`; every scope name and its class (table in docs/google.md, pinned by `scopes.rs` and the guide test), notably `photoslibrary.appendonly` and `photospicker.mediaitems.readonly` (sensitive?) and whether `drive.appdata` is non-sensitive; that a Desktop client needs no registered redirect and takes any `http://127.0.0.1:<port>`; that the refresh response carries `scope`; that the revoke endpoint answers `400 invalid_token` for a token already gone (taken as revoked); that an app in Testing gets seven day refresh tokens and a 100 user cap; that IMAP/SMTP XOAUTH2 (`user=<address>`) is the form Gmail wants. The Gmail API needing to be enabled for IMAP is a guess (the guide marks it).
- **Settings.** `accounts.clients.google` is not offered: the settings writer would drop `client_secret`, `testing` and `byo`, so the owner edits the row by hand. A row written by Settings for another issuer is left as it was.
- **Least privilege.** The refresh does not pass `scope`, so every token carries every granted scope; narrowing a token to a grant's service (Google allows a subset on refresh, unverified) is a later hardening.
- **Follow-up lanes.** *Google calendar source*: syncd's PIM mirror reads CalDAV/CardDAV; graph-calendar adds the source seam, and a `google_api` source (Calendar API `events.list` with `syncToken`, 410 as `AnchorExpired`, People `connections.list` with `requestSyncToken`) follows it through a `google_calendar`/`google_people` bearer from `IssueToken`; the fake needs those endpoints (only the probes exist). *Drive app folder*: a `storage-gdrive` replica over `files` with `spaces=appDataFolder`, `changes.list` with `startPageToken`, resumable uploads (the upload host `www.googleapis.com/upload` is authenticated, so it is the same relay, not a linked origin), MD5 checksums; the fake needs the Drive files API. *Photos*: the append-only upload (two steps: bytes, then `batchCreate`) and the Picker session (`sessions.create`, poll, list picked items) belong to the Photos app lane; D7 keeps that app out of v1.
- **Consumers.** Public additions only (`TokenResponse.scope`, `FamilyProvider::Google`, `FakeProtocol::Google`, `ClientRegistry::traits`); no consumer-used `[dependencies]` changed.

## Lane graph-calendar (syncd, the PIM source seam and the Microsoft calendar mirror)

Lane `graph-calendar`, branch from 177fb33.

- **The seam** (`crates/syncd/src/datasets/pim/source/`): `PimSource` (per account and kind:
  `collections(kind) -> Vec<Found>` with id/URL, display name, colour; `feed(&Found) -> Feed`) and
  `Feed` (per collection: `changes(Option<Cursor>) -> Page<Cursor>` of `FeedChange::Upsert { item,
  content: Option<Vec<u8>> }` and `Delete`, `fetch(id)`, `features()`, `quota()`), with the cursor type
  the source's own (`FeedCursor`: `encode`/`decode` to the journal's anchor text; text that does not
  decode is `AnchorExpired`). `FeedReplica<F>` makes any feed the `Replica` the engine runs over.
  The vdir writer, `PimMirror`, journal, `PullOnly`, rescan and supervisor are unchanged and shared.
  The source is chosen by `choose(kind, candidate)` from the capability's `PimCap.transport`
  (`caldav`/`carddav` -> `DavSource`, `graph` calendar -> `GraphCalendarSource`; any other
  kind/transport pair is `NoSource`, reported once per rescan on standard error), never by provider.
- **CalDAV/CardDAV** is `source::dav` (`DavSource`, `DavFeed` over the existing `WebDavReplica`; its
  upserts carry no content, so the engine still fetches each item). The CalDAV tests are unchanged.
- **Graph calendar** (`source::graph`): `GET /v1.0/me/calendars` (paged; `name`, `hexColor`, else the
  named `color` mapped to an approximate hue) and per calendar `GET .../events/delta` with the
  `@odata.deltaLink` as cursor, `@odata.nextLink` pages, `@removed` as deletes, `410` as
  `AnchorExpired`; whole events arrive in the page, so a fetch costs no request (a fetch after a restart
  is `GET .../events/{id}`). Requests go through `pim_http` (accountd's `OpenAuthenticated` relay),
  as `syncd::graph` does. A calendar's directory is `graph-<12 hex of sha256(id)>` (stable under
  renames; the name is in `displayname`), its files `<UID>.ics`.
- **Converter** (`source::graph::convert::to_ics`, pure, table-tested; output is one VCALENDAR with
  one VEVENT and passes `is_complete`): UID (`iCalUId`, else `seriesMasterId`, else `id`), DTSTAMP/
  CREATED/LAST-MODIFIED, SUMMARY, DTSTART/DTEND (UTC `Z`, `TZID` from a Windows or IANA zone name, a
  `VALUE=DATE` for all-day; a zone not in the table of 40 Windows names is written floating with a
  COMMENT), LOCATION, DESCRIPTION (text body, HTML reduced to text, else `bodyPreview`), RRULE for
  `daily`, `weekly`, `absoluteMonthly`, `relativeMonthly`, `absoluteYearly`, `relativeYearly` with
  `noEnd`/`endDate`/`numbered` (any other pattern: the series master with no RRULE and a COMMENT),
  RECURRENCE-ID for `exception`/`occurrence`, STATUS (cancelled, tentative, confirmed), CLASS, TRANSP,
  ORGANIZER, ATTENDEE (ROLE, PARTSTAT, CN), 75-octet folding, TEXT escaping. Not carried: reminders,
  categories, online-meeting links, attachments, VTIMEZONE blocks (TZID names are IANA).
- Known gaps: an exception is its own file sharing the series' UID with a RECURRENCE-ID, and the master
  has no EXDATE for it, so a reader that does not merge by UID shows the original occurrence too; Graph
  `events/delta` on one calendar is **unverified** (no Graph docs are vendored; `calendarView/delta`
  is the documented form but expands occurrences inside a window and cannot carry a master with its
  rule); `Prefer: outlook.body-content-type="text"` is assumed to apply to it.
- **Scopes**: the Microsoft family asks `Calendars.ReadWrite` for a calendar grant, which covers the
  read this mirror does; refresh uses `.default`. No scope change. But `Family::serves` (porter-core)
  did not let a `graph` endpoint serve Calendar, Contacts or Tasks, so a Microsoft calendar grant had
  no candidate endpoint and accountd's relay refused it (`EndpointNotGranted`): fixed in
  `porter-core/src/family.rs` (three more `graph` arms and table rows), the smallest change.
- **Fake Graph** (`porter-fake-servers::graph::calendar`, additive): calendars, events, `events/delta`
  with `$deltatoken`/`$skiptoken` links and `@removed` stubs, `410 syncStateNotFound`, one event by id;
  `GraphHandle::{seed_calendars, set_calendar, remove_calendar, put_event, remove_event, events}`,
  `expire_delta_tokens` also expires calendar tokens; rig control routes `POST /graph/calendars-seed`,
  `/graph/calendar`, `/graph/event`, `/graph/event-remove`.
- Still pull-only: nothing written to a Microsoft calendar from the vdir (`PullOnly`); Microsoft contacts
  and To Do have no source (`NoSource`).
- **What a `google_api` source must implement** (one module `source::google`, one `Chosen` arm, one arm
  in `AccountMirrors::refresh`; `PimKind` gains `Tasks` for the third, which needs a `DataClass` for its
  grant that porter-core does not have yet, an ask for that lane):
  - Calendar: `collections` = `GET calendar/v3/users/me/calendarList` (id, summary, backgroundColor);
    `Feed::changes` = `events.list` with `syncToken` as cursor (`nextPageToken` pages; `410 GONE` is
    `AnchorExpired`; `showDeleted=true` gives `status: cancelled` stubs as deletes); upserts as VEVENT
    `.ics` bytes (a Google event JSON converter like the Graph one; recurring events with `recurrence`
    RRULE lines and `recurringEventId`/`originalStartTime` for exceptions).
  - Contacts: `collections` = the contact groups of `people/v1` (or one "My contacts" collection);
    `Feed::changes` = `people.connections.list` with `requestSyncToken` and `syncToken` (`EXPIRED_SYNC_TOKEN`
    is `AnchorExpired`; `metadata.deleted` is a delete); upserts as vCard 3.0 `.vcf` bytes.
  - Tasks: `collections` = `tasks/v1/users/@me/lists`; `Feed::changes` = `tasks.list` with `updatedMin`
    and `showDeleted=true` (Tasks has no sync token, so the cursor is the max `updated` time and a
    deleted task is `deleted: true`); upserts as VTODO `.ics` bytes; the mirror's extension and
    envelope for Tasks are `.ics`/VCALENDAR as for a calendar (VTODO inside).
  - Each: `FeedCursor` type, requests through `pim_http` to the family's endpoint
    (`GoogleCalendar`/`GooglePeople`/`GoogleTasks`), the relay adding the bearer.

## Lane agent-login (sign an agent in and out through its launcher, 2026-10-08)

Base `7fc5b0d` (p4-agent-endpoint). Covered (the agreed design, built as agreed; design/31 R7-R9, C5, C6; the launcher side is docket-acp's, S4):

- **Only the agent can log itself in.** `Account.Reauthenticate` on an `AgentLogin` account no longer runs the (empty) review: accountd asks the launcher that registered the account's program. The program is the one the account's agent claim is about (`porter_service::program_of`). Nothing of the login reaches accountd or the bus: no token, no URL, no code, no free text. The only things a launcher can say are `ready`, `failed` plus a word of a closed set (`refused`, `unreachable`, `not_installed`, `timed_out`, `other`: `LoginFault`) or `cancelled`; anything else is `InvalidArgs` and does not use the request up, and the refusal does not echo what was sent. `LoginOutcome::to_wire` / `from_wire` (porter-core `agent_login.rs`) are the one place the words are read.
- **`Peer.RegisterLauncher(programs as)`**, role `AgentLauncher` only (every other role and a stranger: `AccessDenied`). The registration lives as long as the caller's bus connection: `Core::left` (the `NameOwnerChanged` watch that already keeps the roster) frees its programs and ends its pending requests. One launcher per program, first wins: a program held by a live connection is `org.quire.Accounts1.Error.AlreadyRegistered` and none of the list is taken (all or none); registering a program the caller holds is a no-op; a later call adds programs. A program held by a connection that is no longer on the bus (its departure not yet seen) is free: the holder is asked of the bus (`NameHasOwner`) before the refusal. An empty list or a name that is no program is `InvalidArgs`.
- **`Peer.AgentLoginRequested(request s, account s, program s)`** is a signal sent UNICAST to the registrant's unique name (`SignalEmitter::set_destination`); a second `AgentLauncher` connection that did not register sees nothing (test). The request id is `login-<n>` from accountd's counter (`LoginRequestId`, the one id grammar).
- **The sheet.** The Request's sheet opens at `SheetView::Working(provider)` and stays while the agent does it. The method returns the Request path at once, as always; the `Response` is the end of the request. `Ready`: the sheet shows `Done`, `Response` is `Done` (code 0), the account is `ok`. Failed, cancelled, expired, launcher gone, or no launcher: the sheet shows `Failed { fault }` and WAITS for the person (`Retry` asks the launcher again, anything else closes), then the `Response` is the refusal (`Cancelled` is code 1 `Dismissed`; `refused` is `Denied`; `NoLauncher` is the new refusal; the rest `Unavailable`). Closing the sheet while it waits ends the call (`Dismissed`) and does NOT withdraw the request: the request stays pending, and a later `Ready` still sets the account `ok` (the agent finished its own login anyway).
- **`ReportAgentLogin(request s, outcome s, reason s)`**: only the connection that was sent the request may answer it; any other caller, a request id nobody sent, one already answered, expired, or of the other kind (a logout report for a login request) is `org.quire.Accounts1.Error.UnknownRequest`. `ready` sets the account's state `ok` (as `SetAgentState ready` does: saved, `PropertiesChanged`, the shell's `NeedsReauth` bookkeeping, Settings' `Changed("accounts.<id>.state", "ok")`) BEFORE whoever waits is told. `failed` and `cancelled` change no state.
- **Expiry.** A pending request ends `Expired` after a bound on accountd's own clock: `Options::login` (`LoginTiming { clock, bound, tick }`; default the system clock, **ten minutes** (`DEFAULT_BOUND`, a person may be finishing a browser login), checked every second (`DEFAULT_TICK`)). The clock is the `porter_service::Clock` seam, so tests inject one and move it; the sheet shows `Failed { fault: expired }`. A late report is `UnknownRequest`.
- **Sign out.** The Settings `accounts.<id>.sign_out` action on an agent account with a registrant sends `AgentLogoutRequested(request, account, program)` unicast; the launcher answers `ReportAgentLogout(request, outcome, reason)` with the same words; `ready` (done) sets `needs_login`, the others leave the state as it was. With no registrant porter's state goes to `needs_login` as before and the agent's own login is untouched. Settings is told how it went as `Changed("accounts.<id>.sign_out", <word>)` with the word one of `asked` (a launcher was asked), `login_untouched` (no launcher: porter forgot the sign-in, the agent's own login was not touched), `done`, `failed`, `cancelled`, `expired`, `launcher_gone` (`SignOutNews`). This word is the typed result: `ds-settings`' `Set` returns nothing and its `LiveError` has no payload to carry one, so the word rides the `Changed` signal that already reaches the pane. The row's help text now says what it does either way.
- **Typed errors on the bus**: `org.quire.Accounts1.Error.{AlreadyRegistered, UnknownRequest}` (`porter_dbus::LauncherFault`, with `from_error_name`) and the bus's `AccessDenied` / `InvalidArgs`; `NoLauncher` is a `Refusal` (`org.quire.Accounts1.Error.NoLauncher`; `refusal_error_name` table grew to nine) because it is the answer of the Request. `SignInFault` gained `NoLauncher`, `Expired`, `NotInstalled` (sill words them); `LoginEnd` (`Reported | Expired | LauncherGone`) maps onto them.
- **Seams.** porter-service: `Launchers` (`ask_login`, `ask_logout`; a `Waiting` future that does not withdraw the request when dropped), `AccountService::login_agent` / `login_agent_any` (the sheet flow; an app needs its grant, the shell does not), `Host::login_agent` (default `Unavailable`). accountd: `launchers.rs` holds the table and pending requests. porter-client: `Launcher` (`connect`, `register`, `requests` (a `Stream` of `LauncherRequest { kind, request, account, program }`; subscribe before registering), `report_login`, `report_logout`) and `LauncherError`, behind its `dbus` feature. `porter-dbus`: `PeerProxy` methods and signal streams, `BusStream`, `launcher_fault_of`, `is_invalid_args`.
- **Tests**: accountd `tests/agent_launcher.rs` (`a_program_has_one_launcher_first_wins_and_a_dropped_connection_frees_it`, `only_the_launcher_registers_or_answers_and_bad_words_are_invalid`, `a_login_goes_to_the_registrant_alone_and_ready_sets_the_account_ok`, `a_failure_a_cancel_and_an_expiry_leave_needs_login_and_the_sheet_shows_which`, `a_request_the_launcher_does_not_answer_expires_on_accountds_clock`, `with_no_launcher_for_the_program_the_refusal_is_no_launcher`, `retry_on_a_failed_sheet_asks_the_launcher_again`, `only_the_connection_that_got_the_request_may_answer_it`, `closing_the_sheet_does_not_withdraw_the_request`, `a_launcher_that_leaves_with_a_request_pending_ends_it`, `sign_out_asks_the_launcher_and_only_a_done_report_forgets_the_sign_in`, `sign_out_with_no_launcher_forgets_the_state_and_says_the_login_was_not_touched`, `a_sign_out_the_launcher_never_answers_expires_and_a_departure_ends_it`, `the_bus_carries_the_outcome_and_nothing_of_the_login` (a monitor on the bus: no canary, no URL, no `Bearer` in any message; the registry entry and secret store hold nothing)); porter-client `tests/launcher.rs` (three round trips); porter-core `agent_login` unit tests; porter-service `agent_login` unit tests; porter-dbus `launcher` unit tests, the `Peer` introspection test and the checked-in XML.
- **Supersedes** two lines of the P1 section: Settings' `sign_out` no longer "only sets `NeedsLogin`" when a launcher is registered, and sill's "Sign in" on an agent account no longer "runs the (empty) review and leaves the state as the agent said": it asks the launcher.

Left open:

- **sill**: word the three new `SignInFault`s (`no_launcher`: "Open <agent> to sign in", `expired`, `not_installed`) and `Refusal::NoLauncher`; both enums gained variants, so a `match` without a wildcard needs arms. The `Working` sheet for an agent should read "Waiting for <agent>" rather than a provider sign-in. **detent**: shows `accounts.<id>.sign_out` as a plain action; the `Changed` words above are for it to show ("login not touched").
- **docket-acp (S4)**: the launcher side (`Launcher` in porter-client) must subscribe, register, run the agent's own login (browser or device flow, inside the agent), and report. It must not pass the agent's output to the bus. A launcher that restarts re-registers; requests pending for the old connection ended `launcher_gone` when it left.
- A second `Reauthenticate` on the same account while a request is pending sends a second request to the launcher (requests are not coalesced); the launcher may answer the same login for both.
- A login or logout request is audited as asked (lane p5-audit); a report that changes no state (`failed`, `cancelled`, expiry) leaves no line of its own.
- `Standing::Launching` still admits the launcher to `Peer` only; the launcher's role in `dist/callers.toml` is unchanged (never granted by default; the commented row is the machine's to write).

## Lane google-drive (the Drive app data folder, Google Photos upload and the picker, 2026-10-08)

Built, not run against Google: nothing here has touched a real Google client or the network. Everything is tested against the fake Google (porter-fake-servers `google`).

- **`storage-gdrive`** (new crate, depends on porter-core, porter-http, porter-sync, storage-webdav): `GdriveReplica::new(http, base, folder, clock)` (`base` is the `google_drive` row's endpoint, `https://www.googleapis.com/drive/v3`; only its origin is used; `folder` is a path below the app data folder, empty for the folder itself), `.with_page(n)`, `.with_uploads(Uploads::new(simple_max, chunk))` (defaults `SIMPLE_MAX` 4 000 000 and 40 `CHUNK_UNIT`s of 256 KiB). It is porter-sync's `Replica`: the first `changes(Start)` takes `changes.getStartPageToken`, then pages `files.list` (`spaces=appDataFolder`, `trashed = false`), anchors `list:<start>:<page>` and last `changes:<start>`; after that `changes.list` pages, anchors `changes:<token>`; a refused token on a resumed anchor is `AnchorExpired`; a delete or a trash is a tombstone, a move or rename the same id at a new path, a file moved out of the dataset folder a tombstone. `put` is checked first (new: the name in the parent, a file there is `Conflict` `Exists`; existing: `version` against the base, a missing or trashed file is `Deleted`, a moved one `Changed`), then multipart (new) or `uploadType=media` (existing) up to the threshold, a resumable session past it. `remove` is the same check and `DELETE`. `md5Checksum` is the item's hash, `version` its version, `about` the quota, `features()` says `Md5`, ranges and chunked upload present, `Delta::Poll`, app folder scope. Tested by porter-sync's contract suite (the memory reference and the Drive replica over the fake, by multipart and by resumable session: 27 cases) and ten cases of its own (`tests/gdrive.rs`: md5 and size, folders and remote moves, deletes and trashes, the paged first listing resumed by a new process, bad anchors, multipart against resumable on the wire, a stale base, a dataset folder that makes itself, throttling, the bearer on every request).
- **Which store (syncd).** `StorageSupervisor` runs the app folder mirror over `graph_replica` or `gdrive_replica` by the endpoint family its candidate lists (`app_folder_store`: `graph`, else `google_drive`), never by provider name. The dataset is the same (`<account>/storage_app_folder`, owner `org.quire.Files`, `$XDG_DATA_HOME/porter/storage/<account>/`); the folder is now made when the mirror starts (both stores), so the person sees where to drop files. `syncd::gdrive::gdrive_replica(accounts, grant, endpoint, folder, clock)` builds it over `OpenAuthenticated`: the upload host and a session URL are on the API's origin, so no linked origin.
- **Google Photos, upload** (`syncd::datasets::photos::google`, behind `SYNCD_PHOTOS`): `StorageKind::GooglePhotos`, the dataset `<account>/google_photos_upload` (owner `org.quire.Photos`) over a `FolderDataset` of `$XDG_DATA_HOME/porter/photos/<account>/upload/` and `UploadReplica`, a porter-sync `Replica` that is one-way: `changes` is always empty (nothing is pulled), `fetch` is `Gone`, `remove` is acknowledged without a request (a local delete is not a delete in Google Photos), `put` sends the bytes to `/v1/uploads`, makes the album `Quire` (`StorageConfig::google_album`; `albums.create`, app-created) on first need and the item with `mediaItems:batchCreate` into it. A `Ledger` (`$XDG_STATE_HOME/porter/sync/<account>/google_photos_upload.ledger.json`: the album and the SHA-256 of everything sent) means the same bytes under another name, or after a restart, are never sent again; a ledger that exists and cannot be read stops uploads rather than start from empty. A photo's remote id is its media item id (`<id>~<path>` for a copy), its version the SHA-256.
- **Google Photos, picker import** (`PhotosPicker`, from `StorageSupervisor::google_picker(&account) -> Option<Arc<GooglePicker<T>>>`): `start() -> PickerSession { id, picker_uri, poll_every, times_out_in }` (`sessions.create`; the app opens `picker_uri` in the browser), `poll(&id) -> PickerState::{Waiting, Picked}` (`mediaItemsSet`), `import(&id) -> Imported { dir, files, session: SessionEnd::{Deleted, Left} }` (`NotYet` before the person is done; lists `mediaItems?sessionId=` in pages, fetches each `baseUrl` with `=d`, or `=dv` for a video, into `$XDG_DATA_HOME/porter/photos/<account>/picked/<session>/` under a plain unique name, then `DELETE`s the session), `cancel(&id)`. A session id is `[A-Za-z0-9_-]+` or `BadSession`.
- **Grants.** syncd's grant for Google Photos is `Photos`-kind, class Photos (`porter-service` `photos_key`, `sync_key_of`): `OpenAuthenticated` checks the grant's kind against the endpoint family and the token's audience against the provider file's rows of that kind, and the Photos APIs are `photos` rows, so a Storage-kind grant (the Drive app folder) never reaches them. `sync_offers` offers Files for a Google account with the Drive endpoint and Photos for one with the Photos upload endpoint (independently: unticking Drive leaves Photos); `set_sync`, `sync_allowed` and the audit line follow the key. To make the three families reachable by a relay at all, `porter-core` `Family::relay_protocol` is `Http` for `google_drive`, `google_photos_upload` and `google_photos_picker` and `Family::serves` takes them for Storage (Drive) and Photos (both Photos APIs): a behaviour change in a frozen crate with no signature change (tests pin it); the other Google families (calendar, people, tasks, Gmail) are as they were.
- **Scopes verified, no change to the family.** `porter-families/tests/google/drive_photos.rs`: the scopes the sign-in asks (read from the fake issuer's authorize request) are accepted by every Drive call the replica makes and every Photos and Picker call, and each is refused (`403`) with a token that lacks its scope: `drive.appdata` for Drive, `photoslibrary.appendonly` for the upload API and the albums it makes, `photospicker.mediaitems.readonly` for the Picker. That the live services accept those scopes for those methods is unverified.
- **The fake Google** (porter-fake-servers `google`, additive): `FakeGoogle::bind_fixed`/`start_fixed(token)` (fixed bearers, as accountd's relay adds, `GoogleHandle::accept_bearer`), the Drive v3 app data folder (`about`, `changes.getStartPageToken`/`list`, `files.list` with a small `q`, `get` with `alt=media` and ranges, metadata create/patch/delete, multipart, media and resumable uploads, `308` between chunks, `storageQuotaExceeded`), the Photos Library upload/albums/`batchCreate` and the Picker (`sessions`, `mediaItems`, `/dl/<id>` that wants the bearer), and levers: `drive_put_file`, `drive_delete`, `drive_trash`, `drive_move`, `drive_file`, `drive_files`, `drive_version`, `drive_expire_tokens`, `drive_set_limit`, `photos_items`, `photos_albums`, `picker_sessions`, `picker_pick`, `throttle`, `set_drive_knobs`; an MD5 (RFC 1321, the RFC's test vectors) for `md5Checksum`. `rig --google` serves all of it with the issuer's tokens, scope-checked; the levers are in-process only.
- **Tests (syncd, private bus, the real accountd, the fake Google):** `tests/gdrive_bus.rs` (two-way mirror with add, edit, rename, delete both ways through the relay and only the Drive bearer; a two-sided edit is a `Conflict` that `Resolve` settles with `keep_remote` and `keep_local`; a resumable upload of a large file and a multipart one for a small; the changes cursor survives a restart (no listing again) and an expired one reconciles with no re-upload; the dataset follows the grant and `AccountRemoved` wipes; no secret on the bus, in a journal or in debug text) and `tests/gphotos_bus.rs` (the switch off means no Photos dataset, folder or picker; a new file uploads once, a duplicate does not, also after a restart, into one app-created album, a local delete deletes nothing and nothing is read; the picker end to end; the dataset follows the grant; no secret in the ledger, the journal or the picked files). Unit tests in `storage-gdrive`, `datasets::photos::google` and `porter-service/tests/sync_grant.rs`.
- **Unverified (no network):** see the storage-gdrive rows above; for Photos: the Library API's `uploads` (`X-Goog-Upload-Protocol: raw`, `X-Goog-Upload-Content-Type`, the token as the body), `albums` and `mediaItems:batchCreate` (`newMediaItemResults[].status`), that the append-only scope may create an album and add to it, the Picker API's `sessions` (`pickerUri`, `pollingConfig`, `mediaItemsSet`), `mediaItems?sessionId=` (`mediaFile.baseUrl`, `filename`, `type`), `=d` and `=dv`, and that the app-created album survives the person deleting it (then a `400`/`404` makes the next upload create a new one).
- **Not done.** (1)-(5) closed by lane google-followups (auth_origins, `org.quire.Photos1.Picker`, picker-only consent, the Settings words and the guide, the seven days). (6) Video: the upload folder takes videos by extension; the Library API's size limits are not enforced here.
- **Consumers.** `porter-core` (`Family::relay_protocol`, `Family::serves`: behaviour only) and `porter-service` (`photos_key`, `sync_key_of` added; `sync_offers`, `sync_allowed` and `set_sync` behave as before for every account but a Google one) changed with no `[dependencies]` change; `storage-gdrive` is new and used by syncd only.

## Lane google-pim (syncd, Google Calendar, Contacts and Tasks behind the PIM source seam)

Lane `google-pim`, branch from 84980d8. Built and tested against the fake only; nothing here has
touched Google or the network.

- **Covered.** Three `google_api` sources in `syncd/src/datasets/pim/source/google/`, each a `PimSource`
  and a `Feed` through accountd's relay (`pim_http`; syncd holds no token), chosen by `choose` from the
  capability's transport and the kind (`Chosen::{GoogleCalendar, GooglePeople, GoogleTasks}`, one arm
  each in `AccountMirrors::refresh`, which takes a Google API's own endpoint only, never the account's
  first one):
  - *Calendar* (`calendar.rs`, converter `event.rs`): `users/me/calendarList` (id, `summary`, else
    `summaryOverride`, `backgroundColor`; `freeBusyReader` calendars left out) and per calendar
    `events.list`: `nextSyncToken` is the cursor, `nextPageToken` pages (a mid-listing cursor keeps the
    sync token the listing began with), `410` on a cursor is `AnchorExpired`, a cancelled event is a
    delete. A listing from the start asks for no deleted events; one from a token asks (`showDeleted`).
    Directory `google-<12 hex of sha256(id)>`, files by UID (an exception shares its series' UID and is
    named by the event id's hash, as for Graph). Converter: UID (`iCalUID`, else `id`), DTSTAMP/CREATED/
    LAST-MODIFIED/SEQUENCE, SUMMARY, DESCRIPTION, LOCATION, `date` as `VALUE=DATE`, `dateTime` as UTC
    (a single event) or `TZID` and wall-clock (a member of a series, so the rule keeps the zone's clock
    across daylight saving), `recurrence` lines passed through (only RRULE, EXRULE, RDATE, EXDATE, with
    line breaks stripped), RECURRENCE-ID from `originalStartTime`, STATUS, CLASS, TRANSP, ORGANIZER,
    ATTENDEE (CN, ROLE, PARTSTAT, CUTYPE), 75-octet folding and TEXT escaping reused from the Graph
    converter. Not carried: reminders, conference data, attachments, colours per event.
  - *Contacts* (`people.rs`, converter `person.rs`): one collection, "Contacts" (directory
    `google-contacts`); `people/me/connections` with `personFields`, `requestSyncToken=true`, `syncToken`;
    `400` with `EXPIRED_SYNC_TOKEN` is `AnchorExpired`; `metadata.deleted` is a delete. vCard 3.0: UID
    (= resource name), FN (display name, else the parts, else an address, number or organisation), N,
    EMAIL, TEL, ADR (structured, else the formatted text as street), ORG, TITLE, BDAY (`--MM-DD` with no
    year, the RFC 6350 form), NOTE, URL. Files `people_c123.vcf`. Not carried: photos, nicknames,
    relations, other events, custom fields, groups.
  - *Tasks* (`tasks.rs`, converter `task.rs`): `users/@me/lists` (directory `google-<hash>-tasks`) and per
    list `tasks.list` with `showCompleted`, `showHidden`, and from a cursor `updatedMin` and
    `showDeleted`; the cursor is the newest `updated` seen (a mid-listing cursor keeps the time the
    listing began after and the newest so far); `deleted: true` is a delete. VTODO: UID (= id), SUMMARY,
    DESCRIPTION (notes), `DUE;VALUE=DATE`, STATUS NEEDS-ACTION/COMPLETED, COMPLETED (UTC),
    RELATED-TO;RELTYPE=PARENT, DTSTAMP and LAST-MODIFIED (= `updated`). Files `<id>.ics`. `updatedMin` is
    taken as inclusive, so the newest task comes again at every poll with an unchanged version, which the
    engine leaves alone (tested against a locally edited file); a strict `updatedMin` works too.
  - Cursor types `SyncCursor` and `TaskCursor` (`sync.rs`) round-trip through the journal's anchor text;
    text that is not one is `AnchorExpired`. `time.rs` is the RFC 3339 reading and UTC arithmetic.
- **Tasks as a kind.** `porter_core::DataClass::Tasks` (slug `tasks`, after `Contacts`) and
  `VocabVersion` 6 (`store::from_five`, a fixture test of a document stored at 5; the tests for the
  slug and for old slugs still reading). syncd: `PimKind::Tasks` (`.ics`, VCALENDAR with VTODO,
  directory suffix `-tasks`, dataset prefix `pim_task`), `ALL` has three kinds so the supervisor looks
  for Tasks grants. `need_of(Tasks)` asks `Delta::None` (Google's Tasks capability is `delta = "none"`;
  a `Poll` need would never have matched it). A Tasks grant on a CalDAV account is **not** mapped: it is
  `NoSource`. The account's VTODO calendars already mirror as calendars (unchanged), and a second mirror
  of the same collection under `-tasks` would need a component filter in `discover` and would duplicate
  every list, which is more than one line. Also `inferd`: `ai.floor.tasks` (default `on_device`, in
  `dist/inferd.settings.toml`, `CLASSES`, `Policy::proposed`), and `accountd::add::ALL_CLASSES`.
- **The relay for Google's API families.** `Family::relay_protocol` was `None` for `google_calendar`,
  `google_people`, `google_tasks` and `Family::serves` did not let them serve their kind, so accountd would
  have refused a Google PIM grant (`EndpointNotGranted`) exactly as it did Graph before graph-calendar.
  porter-core `family.rs`: the three are `Some(Http)` (their bearer is added by the relay) and serve
  Calendar, Contacts, Tasks respectively (table rows). **Merge note for the Drive lane**: it edits the
  same two matches for `GoogleDrive` (the `None` list in `relay_protocol` and `serves`); the hunks are
  adjacent and a conflict there is a union. The photos families stay unrelayed.
- **FakeGoogle** (new files `porter-fake-servers/src/google/{pim,calendar,people,tasks}.rs`; `google.rs`
  gains four `mod` lines, a `pim` field in `State` and one dispatch line in `serve`): calendarList and
  `events.list` with `syncToken`, `nextPageToken`, cancelled stubs, `410 fullSyncRequired`; `events.get`;
  `people/me/connections` with sync tokens, `metadata.deleted`, `400 EXPIRED_SYNC_TOKEN`; `people/{id}`;
  `users/@me/lists`; `tasks.list` with `updatedMin` (inclusive), `showDeleted`, `showHidden`,
  `showCompleted`, paging; `tasks.get`. Bearer: a token the issuer minted with the service's scope, or one
  given to `GoogleHandle::accept_bearer` (for a test whose tokens come from a fake provider). While no
  calendar or list is set the old probe answers stand. `GoogleHandle::{accept_bearer, expire_sync_tokens,
  seed_calendars, set_calendar, remove_calendar, put_event, remove_event, events, seed_people, put_person,
  remove_person, people, seed_tasks, set_task_list, remove_task_list, put_task, remove_task, tasks}`; rig
  control routes `POST /google/{seed,calendar,calendar-remove,event,event-remove,person,person-remove,
  task-list,task-list-remove,task,task-remove,expire-sync}` and `GET /google/hits` (`porter-rig`
  `control/google.rs`; `Levers` gains `google`).
- **Unverified endpoints and behaviour (no network; check when the owner's client first runs).** Every
  path and parameter above; that `events.list` with a `syncToken` returns cancelled stubs and rejects other
  filters and that a bad one is `410`; that the People sync token's expiry is `400` with `EXPIRED_SYNC_TOKEN`
  in the body (not `410`); that `personFields` accepts exactly the names asked; that a deleted contact
  arrives with `metadata.deleted` and only its resource name; that `updatedMin` is inclusive and
  `showDeleted` lists `deleted: true` tasks; the page sizes (`maxResults` 250 events, 100 tasks;
  `pageSize` 200 contacts); that `dateTime` is the wall-clock time of `timeZone` (which is why a single
  event is written in UTC and only a series uses the zone); `accessRole: freeBusyReader` as the calendars
  that cannot be listed; the quota and rate-limit reasons (`rateLimitExceeded`, `userRateLimitExceeded`,
  `quotaExceeded`, answered `403`) as transient.
- **Gaps.** Pull-only (`PullOnly`): nothing is written back. A series master has no `EXDATE` for an
  exception or a cancelled instance Google sent as its own event, so a reader that does not merge by UID
  shows the original occurrence too (as for Graph). A cancelled instance of a series is deleted from the
  vdir, the master does not learn it. Tasks has no token to expire, so `expire_sync_tokens` leaves it; a
  task completed and cleared on another device stays in the vdir (`showHidden`) as completed. Sill reads
  `.ics` and takes every directory under `vdir/<account>/` for a calendar, so a task list (VTODO only) is
  listed as a calendar named by its directory (`google-<hash>-tasks`): sill needs to skip `-tasks`
  directories or read `displayname`. The client needs `Calendar`/`Contacts`/`Tasks` grants for syncd
  made in Settings (no flow gives them, as for the other PIM grants). The task list "My Tasks" and the
  default calendar are ordinary collections.
- **Consumer arms for the new `DataClass::Tasks`** (read from origin/master, not edited): almanac
  `almanac-service/src/class.rs` `BY_SENSITIVITY: [DataClass; 12]` (add `Tasks` after `Calendar`, 13) and
  `memoryd/tests/it/infer.rs` (an exhaustive `match` at line 777 and a list at 791: `DataClass::Tasks =>
  "tasks"`); docket `docket-reader/src/request.rs` `BY_SENSITIVITY: [DataClass; 12]` (add `Tasks`; without
  it a Tasks-labelled input is classed as the person's own words, `Prompt`); sill, detent, mailo and cua
  match no `DataClass`. Syncd's `PimKind` and `Chosen` are used by no consumer.

## Lane p5-audit (what was released to whom, agent-session ask P5)

- **New `AuditEvent` variants** (additive; the audit file is its own JSONL and carries no vocabulary version, so `VocabVersion::CURRENT` stays 6): `KeyResolved { grant, audience }`, `AgentStateSet { state }`, `AgentLoginAsked { request, program }`, `AgentLogoutAsked { request, program }`, and, reserved for P2 (written since lane p2-handoff), `ProcessCredentialIssued { audience, handoff: Handoff }` and `ProcessCredentialRevoked { audience, reason: CredentialEnd }` (`Handoff`: `memfd`, `tmpfs_file`; `CredentialEnd`: `process_exited`, `grant_revoked`, `account_removed`, `expired`, and since p2-handoff `launcher_gone`).
- **`Peer.ResolveKey`** writes one `KeyResolved` per release; the entry's `app` is the app the grant is for (`org.quire.Agent.<program>` on an agent route), `audience` repeats its name, `account` is the account. The caller (inferd) is not recorded: it is always a porter daemon. `TokenIssued { audience: "resolve_key" }` is no longer written; old files that hold it still read. `accountd::RESOLVE_AUDIENCE` is gone. `KeyDesk::note` takes the audience app, not the caller.
- **`AgentStateSet`** is written by `AccountService::set_agent_state` when the state changed, so it covers `Peer.SetAgentState`, a `ready` login report, a `ready` logout report and Settings' sign out. No `app` (the launcher is one role, not an app). **`AgentLoginAsked`/`AgentLogoutAsked`** are written by accountd's launchers when the unicast signal was sent (`LoginTiming.audit`, new field; `main.rs` wires the file). Only ids, program and state words are recorded, nothing of the login.
- **Reading**: an old line keeps reading (fixtures of every kind master wrote). A kind this build does not know cannot be a variant (serde `other` does not work on adjacently tagged variants that carry a body); `AuditEntry::read_all(text)` skips lines it cannot parse and keeps the rest, and nothing rewrites the file. Nothing in the product reads `audit.jsonl` yet.
- **Settings**: accountd shows no recent activity per account, so no UI was added. When it does, plain wording: KeyResolved "Intelligence used this account's key", AgentLoginAsked "Claude Code signed in" (asked, not necessarily done), AgentStateSet the state change.
- **Open**: a login report that changes no state (`failed`, `cancelled`, expiry) is not audited; a `Once` grant is still not spent by `ResolveKey` (row at the top).
- Tests: porter-core `audit` (`every_event_round_trips`, `the_new_events_pin_their_form`, `lines_written_before_the_new_events_still_read`, `a_reader_skips_a_line_of_a_kind_from_a_newer_daemon_and_keeps_the_rest`); accountd `audit/tests.rs` (`the_file_a_key_release_writes_names_the_grant_and_audience_and_never_the_key`), `tests/resolve_key.rs`, `tests/agent_login.rs` (`set_agent_state_audits_a_change_once_and_a_repeat_not_at_all`), `tests/agent_launcher.rs` (`login_and_logout_requests_and_their_state_changes_are_audited_without_the_login`).

## Lane google-followups (the open Google items, 2026-10-08)

Built against the fakes only; nothing here has touched Google.

- **Vocabulary 7.** `VocabVersion::CURRENT` is 7 (this lane bumps once for items 1 and 6). Rebased on google-pim (master at 6): `from_five` is google-pim's and `from_six` this lane's (identity: `Restriction::signed_in`, `ReauthReason`, `ProviderKind` and the views' `row` are defaulted fields, a document at 6 reads as it was). Fixtures: `a_document_stored_at_vocabulary_five_is_read_unchanged_with_the_sign_in_age_unknown`, `..._six_is_read_unchanged`, `a_restriction_written_before_the_sign_in_date_still_reads`.
- **The seven days (item 1).** `Restriction.signed_in: Option<UnixSeconds>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`), set by the Google family at sign-in (and refreshed by a re-sign-in, `store_renewed`) only when the lifetime is `SevenDays`. `Restriction::expires_at()` and `Restriction::reauth_reason(now) -> Option<ReauthReason>` (porter-core `restriction.rs`, with `SEVEN_DAYS`); `ReauthReason::TestingAppExpired` (closed, `slug()`/`from_slug()`, `testing_app_expired`). Where the reason reaches the sheet host: a new read-only property `ReauthReason` (`s`) on `org.quire.Accounts1.Account` (`""` unless the state is `needs_reauth` and the account is Google-in-testing with an old or unknown-age sign-in), computed on accountd's clock (`Options.login.clock`). **No `Refusal` or `SignInFault` variant was added**: apps compare `Refusal::NeedsReauth` (mailo does), so a new variant would be silently missed; the NeedsReauth signal is unchanged and the shell reads the property on it. Settings: `accounts.<id>.expires` (read-only, only when `expires_at()` is known): "Google signs this account out on YYYY-MM-DD" before, "Signed out by Google on YYYY-MM-DD" when `needs_reauth` (the UTC day); the news hub pushes it with the state. The family's own `ReauthReason` (`TestingExpiry`/`Revoked`) and `GoogleSession::signed_in_at` are untouched (the account service no longer needs `open` to know the date). Not done: the sheet's wording of the property (ds-shell/sill).
- **Picked bytes (item 2).** `CapabilityRow.auth_origins: Vec<AuthOrigin>` (porter-provider; `AuthOrigin` = one exact `host[:port]`, a `*` is refused at parse); `google.toml`'s picker row lists `lh3.googleusercontent.com`. `Tokens.OpenAuthenticated` to a bare origin that is not an account endpoint is accepted when a row of the grant's kind lists it, on the home endpoint's scheme and port (https for the real service; the loopback fake is http), and relays with that row family's bearer (`AccountService::auth_origin_target`, porter-service `relay.rs`); anything else (unlisted, a subdomain, another port, a downgrade, a path, another grant kind, a linked-only origin) is `EndpointNotGranted`. syncd's picker `Http` now dials every origin through `OpenAuthenticated` (never `OpenLinked`); an unlisted `baseUrl` fails as `Unreached` with no request sent. Fake: `GoogleHandle::serve_media()` serves picked bytes on a second origin that requires the bearer and points the `baseUrl`s at it. Tests: `picked_bytes_on_a_second_origin_come_with_the_bearer_when_the_provider_file_lists_it`, `picked_bytes_on_an_origin_the_provider_file_does_not_list_are_refused_and_get_no_bearer`, `an_authenticated_relay_reaches_an_origin_the_file_lists_with_the_credential_and_no_other`, `only_googles_picker_row_lists_an_authenticated_origin_and_it_is_one_exact_host`. Unverified: that Google serves `baseUrl` from `lh3.googleusercontent.com` and wants the bearer on it.
- **Picker-only consent (item 3).** The Google family narrows what it offers by the granted scopes: without `photoslibrary.appendonly` the Photos capability has `upload = Absent`, `albums = None` and no `google_photos_upload` endpoint; without the picker scope `library_read = None` and no picker endpoint (`Granted::allows`/`narrow`); neither is Photos turned off, as before. `sync_offers` therefore offers no Photos backup, and syncd asks for a second, picker-only need (`picker_need`) so the picker runs alone (`google_photos_endpoints` is now `(Option<upload>, Option<picker>)`; the upload dataset starts only with an upload endpoint). Fake issuer: `IssuerHandle::untick(&[scopes])` (granular consent). Tests: `a_person_who_grants_only_the_picker_gets_no_upload_to_offer` (and the upload-only, both and neither cases), `a_google_account_where_only_the_picker_was_granted_offers_no_photos_backup`, `a_person_who_granted_only_the_picker_gets_the_picker_and_no_upload_dataset`.
- **The picker on the bus (item 4).** `org.quire.Photos1.Picker` on syncd's name and object (`/org/quire/Sync1`), second interface in `dbus/org.quire.Sync1.xml`: `Start(account s) -> (session s, picker_uri s, poll_s u)`, `Poll(account s, session s) -> s` (`waiting`/`picked`), `Import(account s, session s) -> as`, `Cancel(account s, session s)`; `account` is the account directory name of a dataset name. Caller: the connection's, via `Callers` as Sync1; only an app in the Photos owners (`org.quire.Photos`) may call, Settings, daemons and others get `Denied`, a stranger `AccessDenied`. Errors: bad word `InvalidArgs`; no Picker for the account (unknown, switch off, picker scope not granted) `NoFittingAccount`; `org.quire.Photos1.Error.NotYet`, `...NoSuchSession`; Google unreachable `Unavailable`. Served by `service::serve_picker` only when `SYNCD_PHOTOS=on` (main.rs); `StorageSupervisor::pickers()` is the shared registry, refreshed every tick. porter-client has no syncd client, so there is no client side yet (porter-dbus `PhotosPickerProxy` is what the Photos app uses). Tests: `crates/syncd/tests/photos_picker_bus.rs` (three), the introspection test.
- **Words (item 5).** A Google account's `accounts.<id>.sync.photos` row reads "Upload new photos from a folder on this computer to Google Photos" with a help naming the upload folder and that nothing else is read or deleted; a Microsoft one stays "Back up photos". docs/google.md has "What porter does with each Google service" (a guide test pins its words).
- **Sheet views carry the provider (item 6).** `ProviderRow` gains `auth: ProviderKind` (`Service` default, `AgentLogin` from the provider file's `auth.kind = agent_login`); its `label` is the display name. Every `SheetView` that names a provider now also carries `row: Option<ProviderRow>` (`SignInView.row`, `ReviewView.row`, and the `row` field of `BrowserWait`, `ShowCode`, `Failed`), and `SheetView::Working` is now a struct variant `Working { provider, row }` (it was `Working(ProviderId)`). `row` is `Some` whenever the provider is in the sheet's provider list (always, in accountd), `None` only for a view written before vocabulary 7 or an unlisted id. Consumer arms: sill matches `SheetView::Working(p)` -> `Working { provider, row }`, and can drop its agent table: `row.auth == ProviderKind::AgentLogin`, name `row.label`.
- **Consumers.** porter-core public types changed (`Restriction` has a new field, so a literal `Restriction { .. }` needs `signed_in` or `..Restriction::none()`; `SheetView::Working`, `ProviderRow`, `SignInView`, `ReviewView`, `BrowserWait`, `ShowCode`, `Failed` as above; `ReauthReason`, `ProviderKind` new); `Refusal` and `SignInFault` are unchanged. `CapabilityRow` has a new defaulted field. See the report for the enum arms.


## Lane p2-handoff (a scoped, revocable API key for a spawned agent process, agent-session ask P2)

Covered (acp-sessions.md section 8 P2, P6, P8; section 12 R3; for agents that cannot take a base URL, as Gemini CLI is declared; **the metered route is P4, inferd as the endpoint, and is the default**):

- **Methods** (`org.quire.Accounts1.Tokens`, `dbus/org.quire.Accounts1.xml`): `IssueProcessCredential(grant s, program s, target s) -> (s, v)` and `RevokeProcessCredential(id s)`, and the signal `ProcessCredentialRevoked(id s, reason s)`. `target` is `memfd` or `tmpfs_file` (`porter_core::audit::Handoff`, a closed set; `Handoff::word`/`from_word`). The reply is the credential id (`ProcessCredentialId`, `cred-<n>`, a porter-core newtype with the id grammar) and the handle in a variant: a file descriptor (`h`, a sealed memfd made like `Peer.ResolveKey`'s: `sealed_key`) or a path (`s`). The key is in no reply body, signal, audit line or log.
- **Who**: `CallerRole::AgentLauncher` only (P8; the role existed), and only for a program this connection registered with `Peer.RegisterLauncher` (a registration lives as long as the connection). Every other role and a stranger: `AccessDenied` (both methods). `RevokeProcessCredential` ends only a credential the same connection was issued.
- **What**: a grant that is an `Allow`, kind Llm, held for the app `org.quire.Agent.<program>` (P4's audience convention, any isolation), on an account whose `AuthKind` is `ApiKey` or `OAuthMintsKey` (the two that file an `ApiKey` secret), not turned off for Llm and not `NeedsReauth` (the same check as `ResolveKey`, now one function `keys::usable`). **Never a `Once` grant**: a once-grant is spent by its first use, but a process holds its key for its whole life, so a consent that said "once" would hand over a key that works until the process ends, and the second turn of the agent would find the grant gone and the key still in the child. `Always` was the only scope accepted until lane session-scope added `GrantScope::Session` (the match in `handoff.rs` has no catch-all arm, so the new arm was a compile error there): a `Session` grant is accepted for a session the same connection holds open.
- **Errors** (names under `org.quire.Accounts1.Error.`, plus the bus's own): `NotRegistered` (the connection holds no registration of that program), `OnceGrant`, `NotAKeyAccount` (OAuth, an agent that signs itself in, a local runtime), `UnknownCredential` (revoke), all new `LauncherFault`s; `UnknownGrant` (no such grant, a `Deny`, or the account is gone), `AudienceNotGranted` (a grant for another app, another program's app or another kind), `Denied` (turned off), `NeedsReauth` (also a key no longer filed), `Unavailable` (no key desk, no `$XDG_RUNTIME_DIR` for `tmpfs_file`, a file or memfd that could not be made); `org.freedesktop.DBus.Error.InvalidArgs` for a malformed grant, program, id or target; `AccessDenied` for the wrong caller.
- **Lifetime**: a credential lives until `RevokeProcessCredential`, the launcher's connection leaving the bus, its grant being withdrawn (Settings, `Grants.Revoke`) or its account being removed. The last two are found after every change to the registry (`Core::publish` calls `end_credentials`), so every path that changes the registry covers them. accountd unlinks a `tmpfs_file` credential's file and its directory; for a `memfd` it cannot reach the child's copy: it tells the launcher in the unicast signal `ProcessCredentialRevoked(id, reason)`, and **the launcher must end the process**. No signal when the launcher itself ended it (it knows) or left (nobody is there).
- **Files** (`credentials.rs`): `$XDG_RUNTIME_DIR/porter/agent/<credential id>/key`, the directories 0700, the file 0600 and created new (`create_new`: never over an existing file or through a link); `$XDG_RUNTIME_DIR` comes from the environment in `main` (`Options::runtime_dir`, none refuses `tmpfs_file` as `Unavailable`). accountd clears `porter/agent` when it starts (a file left by an accountd that died is a credential nobody can end; credential ids restart at 1) and refuses to write into a directory that is already there. Whether the runtime directory is a tmpfs is the system's word, not checked (it is under systemd and on the jail).
- **Audit**: `ProcessCredentialIssued { audience, handoff }` and `ProcessCredentialRevoked { audience, reason }` (the entry's `app` is the grant's app and `account` the account; no grant id, no credential id, no path). Reasons: `process_exited` (the launcher called `RevokeProcessCredential`: it says the process is over or killed), `launcher_gone` (new `CredentialEnd`; the connection left), `grant_revoked`, `account_removed`; `expired` is unused. No `KeyResolved` is written for a handoff (that line means `Peer.ResolveKey`). New `KeyDesk` methods `note_handoff` and `note_handoff_end`.
- **porter-client**: `Launcher::issue_credential(&GrantId, &AgentProgram, Handoff) -> ProcessCredential`, `revoke_credential(&ProcessCredentialId)`, `revocations() -> Revocations` (a stream of `Revoked { id, reason }`; subscribe before issuing). `ProcessCredential` gives the id, `handle()` (`CredentialHandle::Memfd(OwnedFd)` or `TmpfsFile(PathBuf)`), `read_key()` (by `pread`: a plain `read` on a received memfd moves the position every holder of the descriptor shares, and the child inheriting it would find the end) and `child_key(&EnvName, Delivery)`: `Delivery::Value` gives the key text for the program's own key variable (`AgentCap.key_env`) to set in the child after `env_clear`; `Delivery::File { child_fd }` gives `<KEY_ENV>_FILE` = `/proc/self/fd/<child_fd>` plus a duplicate descriptor to place at that number (`Inherit`), or the tmpfs path. `LauncherError` gained `NotRegistered`, `OnceGrant`, `NotAKeyAccount`, `UnknownCredential` and `Refused(Refusal)`.

**The launcher's duties** (docket-acp; also in the `credential` module docs): set the key only in the child, after `env_clear`, never in its own environment, a log or a command line; watch `Launcher::revocations()` and end the process when accountd ends its credential (ask it to quit, kill it after a grace period of the launcher's choosing: accountd cannot, and "told and killed after a grace period" is the launcher's); call `revoke_credential` when the process exits or is killed; one credential per child, for the program it was issued for; a launcher whose connection drops loses all its credentials at once and then has no way to be told, so it should kill its children when its bus connection fails.

**Residual risk, plainly**: the child holds its own key. It, and anything it spawns, can read it and send it wherever its network allows; the sandbox's network rules are the mitigation, and a short-lived or spend-limited key (P3) the better answer. A handed-off key is **outside porter's meter**: `ai.spend.*` caps count only what passes inferd, and inferd's data-class floors (`ai.local_only`, the class's floor) are not applied to a key a process holds. **The metered, key-less route is P4** (inferd as the agent's model endpoint, a dummy token, the real key attached inside porter), for every agent that takes a base URL; use P2 only for an agent that cannot. A memfd keeps the key out of `/proc/<pid>/environ`; `Delivery::Value` puts it back there, readable by processes of the same user, and is the only way to a program that reads nothing but its key variable. Not checked: that the key account's `Subject::Agent(program)` claim names the program (the grant is the person's word, as in P4).

Not done: an expiry (`CredentialEnd::Expired`; a credential lives until one of the ends above); a cap on credentials per launcher; the launcher side in docket (S4); the product facts (which agents read a key from a file or helper, `apiKeyHelper`) are unverified, as the p1 list says; Settings shows no credential in use.

Tests: porter-core `audit` (`the_bus_words_of_a_handoff_and_an_end_are_their_audit_words`, `the_new_events_pin_their_form`, `every_event_round_trips`), `process_credential` (`an_id_is_the_id_grammar_and_so_one_safe_path_segment`, `it_reads_and_writes_as_its_text`); porter-dbus `introspection` (`checked_in_introspection_matches_the_interfaces`, `every_accounts_member_is_declared`), `launcher` (`every_fault_has_a_distinct_valid_name_that_reads_back`); accountd `credentials/tests.rs` (six: modes, unlinking, owner, memfd seals, stale clearing, planted directory), `tests/process_credential.rs` (`a_memfd_credential_is_the_key_on_a_sealed_descriptor_and_the_key_is_nowhere_else`, `a_tmpfs_credential_is_a_0600_file_in_a_0700_directory_and_revoking_it_unlinks_it`, `only_the_launcher_of_a_program_may_ask_for_its_key`, `a_grant_that_does_not_back_a_process_credential_is_refused_with_its_reason`, `the_tmpfs_way_needs_a_runtime_directory_and_the_memfd_way_does_not`, `another_launcher_cannot_end_a_credential_it_was_not_issued`, `a_launcher_that_leaves_ends_its_credentials_and_only_its_own`, `revoking_the_grant_ends_the_credentials_under_it_and_tells_their_launcher_alone`, `removing_the_account_ends_its_credentials_with_that_reason`, `revoking_the_grant_from_settings_ends_it_too`), `tests/file_keys.rs` (`the_real_daemon_hands_the_launcher_a_key_by_memfd_and_by_file_and_logs_none_of_it`: the real binary, its stderr and `audit.jsonl` scanned); porter-client `credential` (three) and `tests/process_credential.rs` (`a_memfd_credential_sets_a_child_up_by_value_or_by_an_inherited_descriptor`, `a_tmpfs_credential_is_a_private_file_that_ending_it_removes`, `accountd_ending_a_credential_reaches_the_launcher_as_a_revocation`, `a_refusal_is_a_typed_error_and_a_key_is_never_part_of_it`).

## Lane session-scope (a grant that lasts one launcher session, "This session only", agent-session ask S4)

Covered (base `dc7dd65`; the launcher side is docket-acp's):

- **The scope.** `GrantScope::Session(LauncherSession)` (porter-core; serde `{"session": "<id>"}` beside `"once"` and `"always"`, which still read). `LauncherSession` is a newtype with the id grammar (`[a-z0-9]` then `[a-z0-9._-]`, at most 64 bytes), chosen by the launcher; an ACP session id that is not in it (capitals, longer) is the launcher's to map (for instance to a lowercase hex digest): accountd only ever sees the mapped id. **`GrantScope` is no longer `Copy`** (the id is a string): `Scripted` in porter-fake lost `Copy` with it. `GrantScope::{word, session, from_words}` carry the scope where it is one string. `VocabVersion` is **8** (`from_seven` is a no-op migration; the test holds a frozen vocabulary 7 document as literal text, and a session grant round-trips through `Persisted`). A starting accountd removes every session grant its store holds (no session is open in a new process; each is audited as `session_grant_ended`).
- **Sessions** (`Peer`, `AgentLauncher` only, and only a connection that registered a program): `BeginSession(session s)` and `EndSession(session s)`. A session belongs to the connection that began it. `BeginSession` of a session another live connection holds is `SessionTaken` (first wins; it stays taken until that connection's departure is seen), of one this connection holds is a no-op; `EndSession` of a session not open or not this connection's is `UnknownSession` and ends nothing; a malformed id is `InvalidArgs`; an unregistered connection is `NotRegistered`. Both are new `LauncherFault`s (`org.quire.Accounts1.Error.{SessionTaken, UnknownSession}`); porter-client's `LauncherError` has them too.
- **Asking.** `Peer.RequestAgentGrant(program s, kind s, class s, session s, parent_window s, options a{sv}) -> o` (a Request object answered as `Manager.Choose`'s is: `SheetKind::Choose`, the chosen candidate and its grant, code 1 for a closed sheet, code 2 plus `refusal` for a refusal). Today `Manager.Choose` cannot reach it: the launcher is not an acting role (`Standing::Launching`), so this is the launcher's route to the consent sheet. The grant is held by the app `org.quire.Agent.<program>` (the P4 and P2 audience), `kind` must be `llm` (anything else is `InvalidArgs`: the agent's grant is its key), interactive use, the data class the launcher names; the program must be one the connection registered (`NotRegistered`); `session` is empty or an open session of this connection (`UnknownSession` otherwise). The sheet's `ConsentAsk` has a new field `session: Option<LauncherSession>` (absent from the JSON when none, so an ask an earlier build wrote still reads): **the sheet may offer "This session only" exactly when it is `Some`**, and an answer whose scope is a session other than the ask's (or any session scope on an ask without one, an app's own `Choose` included) is treated as dismissed and records nothing (`choose::settle`). The sheet's title is the ask's `app` (the agent's `org.quire.Agent.<program>`, shown through `AppNames`); no separate title field. "Add Account…" in a session ask ends the request dismissed (the account it would add gets an `Always` grant, not the session one); a person adds the account in Settings and asks again. The options are not data: sill draws Once and Always itself, so the new option needs a sill change (see the arms below).
- **Ending.** When a session ends (`EndSession`, or its connection leaving the bus, `Core::left` now async): the session is removed from the table first (a new `IssueProcessCredential` or `RequestAgentGrant` under it is refused), then every process credential under a grant of that session ends `session_closed` (`CredentialEnd::SessionClosed`, the file unlinked, audited as `process_credential_revoked`; `ProcessCredentialRevoked(id, "session_closed")` goes to the launcher only on `EndSession`, since a launcher that left is not there to hear it), then the session's grants are removed (`AccountService::end_session_grants`; audited as `AuditEvent::SessionGrantEnded { grant, session }` with the grant's app and account, and saved), then the registry is published. The order keeps a credential from ever being reported `grant_revoked`. A connection that leaves ends its session credentials `session_closed` and its other credentials `launcher_gone`. A session grant the sheet makes while its session closes (the sheet was open) is removed again and the request ends dismissed (`settle_session`).
- **P2.** `IssueProcessCredential` accepts `Always` and a `Session` grant whose session this connection holds open (`UnknownSession` otherwise); `Once` is still `OnceGrant`. Re-checked after the key is read and the credential made: a grant that went, or whose session closed meanwhile, takes the credential back.
- **P4 (inferd's agent endpoint).** `Peer.Verdicts` reads the registry's grants, so a session grant reads as `granted` while it is open and `ask` once the session has ended (its grant is gone); inferd asks per request (`agent/account.rs`), so a request after the session is 403. What changed: a row's details carried `scope` as the serde string, which a session scope is not; they now carry the scope's word (`once`, `always`, `session`) and, for a session, `session` beside it, and inferd's `verdict_of` puts them back with `GrantScope::from_words` (a `session` word with no session, or a session on another word, makes the row grant nothing). An older inferd would drop the row of a session grant (it reads `session` as no scope): the daemons release together.
- **Settings.** A session grant's row reads "<App> can use <Service> (this session)" (the service word is `service_label`, and accountd's `AppNames` names the agent app from its provider file, so the agent's key row reads "Claude Code can use Language model (this session)"; an agent program with no `agent_login` provider file reads "org.quire.Agent.<program> can use ..."); and a session grant coming or going is said as `Changed("accounts.<id>.grant.<grant>", "added" | "removed")` so a pane that follows `Changed` reads the schema again and the row appears and disappears with the session (other grants' rows are not announced, as before). Revoking the row in Settings ends the grant and its credentials as any grant (`grant_revoked`), and the session stays open.
- **porter-client.** `Launcher::begin_session(&LauncherSession)`, `end_session(&LauncherSession)`, `request_agent_grant(&AgentProgram, DataClass, Option<&LauncherSession>) -> Candidate` (subscribes to the Request first and closes the sheet if the call is dropped; the candidate's `grant` is the id to give `issue_credential`; a closed sheet is `Refused(Dismissed)`), and `LauncherError::{SessionTaken, UnknownSession}`.
- **Tests.** porter-core: `store` (`a_document_stored_at_vocabulary_seven_is_read_unchanged_and_its_scopes_still_read`, `a_session_grant_survives_the_document_and_keeps_its_form`), `audit` (the new words and events), `launcher_session`, `tests/round_trip.rs` (`a_session_scope_round_trips_with_its_session_and_the_old_scopes_keep_their_words`, `a_consent_ask_names_its_session_only_when_it_has_one`, `the_vocabulary_is_version_eight`); porter-dbus codec (a session grant over the D-Bus shape), `introspection` (ten Peer methods), `launcher` faults; porter-service `tests/session_scope.rs` (`the_ask_carries_the_session_only_when_the_request_was_made_in_one`, `a_session_answer_is_recorded_for_the_session_the_ask_carried`, `a_session_scope_the_ask_did_not_offer_makes_no_grant`, `once_and_always_still_work_in_a_session_ask`, `ending_a_session_removes_its_grants_alone_and_audits_each`, `add_account_in_a_session_ask_is_dismissed_not_granted_for_good`); accountd `tests/session_scope.rs` (bus: `a_session_belongs_to_the_connection_that_began_it_and_only_a_registered_launcher_may`, `a_session_grant_is_asked_only_for_an_open_session_of_the_launcher_and_its_own_program`, `the_sheet_offers_this_session_only_just_when_the_request_carries_an_open_session`, `ending_the_session_removes_its_grant_and_ends_the_credentials_under_it`, `a_launcher_that_leaves_closes_its_sessions_and_nobody_else_hears_of_it`, `a_session_that_closes_while_its_sheet_is_open_leaves_no_grant`, `a_process_credential_takes_an_open_session_grant_and_still_refuses_a_once_grant`, `inferds_verdict_reads_a_session_grant_as_allowed_while_it_is_open_and_not_after`, `settings_shows_a_session_grant_for_this_session_and_the_row_goes_with_it`, `a_session_grant_in_the_store_does_not_survive_a_restart`), `hub` (`a_session_grants_row_is_said_to_come_and_go_and_another_grants_is_not`); inferd `cloud::accountd` (`a_row_is_a_verdict_and_a_malformed_one_grants_nothing` with the session rows); porter-client `tests/process_credential.rs` (`a_session_grant_backs_a_credential_and_ending_the_session_ends_both`).
- **Consumer arms** (read-only grep of `origin/master`). *sill*: `sill-overlays` `accounts_sheet/events.rs:21` maps `AllowScope::{Once, Always}` to `GrantScope`, `sill-model` `accounts/sheet/command.rs:28` (`Do::Allow(GrantScope)`) and `step.rs:225` carry it, so a third button "This session only" (shown when the ask's `session` is `Some`, answering `GrantScope::Session(id)`) needs an `AllowScope::Session` and an arm there; `GrantScope` is not `Copy` now, so any `#[derive(Copy)]` on a type holding it (`Do`, `SheetAction`) must drop `Copy`; the two `ConsentAsk { .. }` literals in `sill-model/.../tests/step.rs:60` and `sill-overlays/.../accounts_sheet/tests.rs:81` need `session: None`. *detent*: none. *mailo*: `mail-app/src/ui/add_account/host.rs` implements `Sheets::consent(ConsentAsk, ..)` only (no field read). *docket* and *cua*: their `GrantScope` is `docket_core`'s own; `actions-mcp` tests build porter `Grant`s with `GrantScope::Always` (no change). No consumer matches porter's `GrantScope` exhaustively except sill's mapping above.

Left open:

- **sill**: the "This session only" button and the arms above; until then a session ask is shown with Once and Always only, which the person can answer (nothing breaks, the option is just not shown).
- **docket-acp (S4)**: map the ACP session id to a `LauncherSession` (lowercase id grammar), `begin_session` when the ACP session starts and `end_session` when it closes, `request_agent_grant(program, class, Some(&session))` for the key, `issue_credential` under the returned grant, and end the process on `revocations()` (`session_closed`). A launcher that dies loses its sessions and cannot be told; it should kill its children when its bus connection fails (as the p2 section says).
- Sessions do not survive an accountd restart (a session grant in the store is dropped at start); a launcher that outlives an accountd must `begin_session` again and ask again. The launcher is not told that accountd restarted other than by its calls failing.
- A session grant does not expire by time; a launcher that never ends a session and never leaves holds it. There is no cap on sessions or session grants per launcher.
- `Peer.RequestAgentGrant` is the Llm kind only and interactive use only (the P2 and P4 grant); other kinds for an agent are not asked here.
- The Settings `Changed` word for a session grant is `added`/`removed` on the grant's row key; `ds-settings` has no signal for "the key set changed" (as for accounts), so a pane must read the schema again on any `Changed`.

## Lane hunt-fixes (the porter bugs the accounts-screen hunt found, base `5b34d8b`, 2026-10-08)

- **1. The password is tried at add.** `porter-families` generic `login.rs`: before the review (and on Reauthenticate) `porter_proxy::relay` runs the relay's own login against the first incoming endpoint (IMAP, POP3) over a probe app stream that notes the relay's first write (sent only after it authenticated) and refuses it; 15 s in all. Refused is `SignInFault::Refused`; connect, TLS or timeout is `Unreachable`; a server that speaks no usable login is `Unreadable`. DAV already sent a PROPFIND with the password (`dav::discover`), so it is untouched; JMAP reads its session with the credential; a bearer token is not tried. `generic` now depends on `porter-proxy/tls` and `tokio`; `GenericProvider::with_connect(impl Connect)` is the seam (default: the platform's roots, read on first use, so accountd needed no change).
- **2. The same login twice.** Chose the refusal: the sheet ends at the review with the new `SignInFault::AlreadyAdded` (answers `Refusal::Unavailable`), the stored account untouched. Same provider, same logins, same hosts; an account with no login (API key) is never a duplicate. `store_new` also refuses it (a second sheet racing the first). sill needs an arm for the new fault. An agent account is one per agent provider: a second add of the same agent ends `AlreadyAdded` too, whatever its sign-in shows (an agent signs itself in to the one plan its own login holds; `add_store::duplicate`).
- **3. An agent with no launcher.** `AccountService::set_launcher_roster(Roster)`; the add sheet's Start step ends `SignInFault::NoLauncher` (`Refusal::NoLauncher`) for an `agent_login` provider whose program has no launcher, before anything is stored. accountd gives it its registrations in `serve_with` (`Host::use_launcher_roster`). A service never told (an app hosting porter in process) is as before. Sign in again on an already-stored `needs_login` agent account already asked the launcher and answered `NoLauncher` with none (`login_agent`): unchanged.
- **4. Provider order.** `add_flow::ordered`: named rows by lower-cased label, then the generic rows with `generic-imap` first.
- **5. App names on sheets.** `ConsentAsk::app_label` and `ReviewView::allow_label` (`Option<AppLabel>`, serde default, left out when absent), filled by `BusSheets::with_names(AppNames)` from the same table Settings reads (caller table, desktop entry, agent provider label); an app nothing names stays `None` (the id is not a name). The other bare `AppId` sill gets is `ReviewView::allow` (the "Add, and allow ..." button); `Purpose::Add::allow` is internal. **Vocabulary 9** (`from_eight` is a version move only): this lane's `AlreadyAdded` and the two labels.

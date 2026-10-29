# Conventions

porter follows the program's shared conventions, whose text lives in quire's `CONVENTIONS.md`
(identical in quire, sill, shell-host, palmrest and detent): types, traits, effects, errors,
tests, comments, change discipline, borrowing. Read it first. `ARCHITECTURE.md` here adds the
crate map, the one-home table, the traits, the recipes and the repo rules; where the two
disagree, `ARCHITECTURE.md` wins for porter.

What porter adds or decides differently, each with its reason:

1. **Closed sets are enums without `Word`.** porter does not depend on quire's `ds-core`: the
   account core must build for mailo on macOS and Windows and for other desktops, below the
   design system. A closed set's stable slug is its serde `snake_case` form (the same slug in
   files, on the bus and in the consent store); nothing hand-writes `slug`, `parse` or `ALL`,
   and labels a person reads belong to the UI that draws them (detent, accounts-ui).
2. **Async in traits is native, never the `async-trait` crate.** A seam is
   `-> impl Future<Output = ...> + Send` (return-position `impl Trait`) or a plain `async fn` in
   its implementations, so every future can cross a multi-threaded runtime and dispatch is
   static. Only a trait used as `dyn` (an `Arc<dyn Machines>`) returns a hand-boxed
   `Pin<Box<dyn Future<Output = T> + Send + 'a>>`.
3. **`todo!()` bodies exist only while an interface is frozen and its behaviour is not
   built.** Each one is listed in `FINDINGS.md` with the work that removes it. This departs
   from the shared rule "no `todo!()` stub on master"; the freeze decides when it lands.
4. **Nothing ambient below the daemons.** The clock (`porter_service::Clock`), the secret store,
   the consent prompter, providers and transports are passed in; only `accountd` and `syncd`
   read the system clock, the environment or the bus.
5. **One integration-test executable per crate.** A crate's integration tests are modules of
   `crates/<crate>/tests/it/main.rs` (`mod <topic>;`, one `mod common;` there, helpers used as
   `crate::common`); fixtures stay in `tests/fixtures/`. A test that starts its own test
   executable again as a fake child does so by module path, so it is a
   module too. Every `tests/it/main.rs` carries the one-line guard test
   `test_kit::guard::every_module_is_declared(env!("CARGO_MANIFEST_DIR"), "tests/it",
   include_str!("main.rs"))`, which fails when a file or module directory there is not declared.
   A separate `tests/<name>.rs` with a `[[test]]` entry in the crate's `Cargo.toml` and a one-line
   reason is for a test that sets process-wide state or needs a `required-features` (or a
   feature-less build) the rest must not: porter-client `socket_accounts` builds without
   stoker. Dependencies build without debug info (`[profile.dev.package."*"]`).

Type rules (the owner's, 2026-10-28; quote them in review):

- `bool` is fine for fields, returns, locals and private fns. An enum only where a public fn
  would read `f(true, false)`, a third state is realistic, or the value is persisted or on the
  bus.
- A newtype only when it holds a rule (parsed or validated), a unit, or an id that must not be
  mixed up with another.
- An enum for a closed set the code branches on. A set that grows (issuers, files, ports) is
  data, in a provider file or a table.
- Behaviour that varies by case is a trait with default methods. Generics by default;
  `Box<dyn>` or `&'static dyn` when the case is picked from config, a file or the bus. The
  stored or bus id stays as it is, and one function maps it to the impl.
- One fact has one type: no twin enums (use `#[serde]` attributes or `From`), and a two-way
  outcome is a `Result` or an `Option`.
- A data format never changes with a refactor: every persisted or bus value round-trips byte
  for byte, and a type that was stored and becomes another gets a test that the stored text is
  read and written unchanged.

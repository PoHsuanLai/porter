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
2. **Async seams use `-> impl Future<Output = ...> + Send`** (return-position `impl Trait`),
   so implementations write `async fn` and every future can cross a multi-threaded runtime.
   Closed sets of implementations are enums implementing the trait (`accountd`'s
   `FamilyProvider`, `inferd`'s `AdapterModel`), never `dyn`.
3. **`todo!()` bodies exist only while an interface is frozen and its behaviour is not
   built.** Each one is listed in `FINDINGS.md` with the work that removes it. This departs
   from the shared rule "no `todo!()` stub on master"; the freeze decides when it lands.
4. **Nothing ambient below the daemons.** The clock (`porter_service::Clock`), the secret store,
   the consent prompter, providers and transports are passed in; only `accountd`, `syncd` and
   `inferd` read the system clock, the environment or the bus.

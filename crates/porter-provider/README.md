# porter-provider

Providers as data, for [porter](https://github.com/PoHsuanLai/porter): the TOML declaration format
and its parser, the set of installed providers, and the `Provider` trait each protocol family
implements. Pure: the caller reads files and runs futures.

The files porter ships (in `providers/`) are compiled in, so an app has the same providers with
nothing installed:

```rust
use porter_core::ProviderId;

let providers = porter_provider::shipped();
let id = ProviderId::parse("nextcloud").expect("an id");
let nextcloud = providers.get(&id).expect("porter ships a Nextcloud file");
assert_eq!(nextcloud.id.as_str(), "nextcloud");
```

## License

MIT OR Apache-2.0.

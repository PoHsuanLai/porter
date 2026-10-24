# porter-core

The pure vocabulary of [porter](https://github.com/PoHsuanLai/porter), the desktop's account and
consent service: accounts, the capability vocabulary and how a need meets an offer, consent,
credentials' filing, and the wire protocol. No runtime, no file system, no I/O. The time is one
seam, `clock`, whose `SystemClock` reads the wall clock only when a daemon calls it.

Every id is parsed once where it enters, so the rest of the code holds a value that is already
safe in a file name and, after `object_segment`, in a D-Bus object path:

```rust
use porter_core::{AccountId, object_segment};

let id = AccountId::parse("work.nextcloud").expect("a well-formed id");
assert_eq!(id.as_str(), "work.nextcloud");
// On the bus the id is one path segment: `.` and `-` become `_`.
assert_eq!(object_segment(&id), "work_nextcloud");
// Capital letters are not in the grammar.
assert!(AccountId::parse("Work").is_err());
```

## License

MIT OR Apache-2.0.

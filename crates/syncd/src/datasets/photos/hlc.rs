//! Hybrid logical clocks: the stamp on every metadata field (design/31 §6.2 "Photos: CAS
//! originals + HLC manifest"). A stamp orders by wall milliseconds, then a counter, then the
//! device's name, so two devices never make equal stamps and "the later write wins" is one
//! total order every device computes alike.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::atomic::{AtomicU64, Ordering};

/// A device's name: `[a-z0-9]{1,16}`, one path component and no `-`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    /// The name, if it is one.
    pub fn parse(name: &str) -> Option<Self> {
        let ok = !name.is_empty()
            && name.len() <= 16
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        ok.then(|| Self(name.to_owned()))
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One stamp. The derived order is the clock's: wall, counter, device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hlc {
    wall: u64,
    counter: u32,
    device: DeviceId,
}

impl Hlc {
    /// A stamp from its parts.
    pub fn new(wall: u64, counter: u32, device: DeviceId) -> Self {
        Self {
            wall,
            counter,
            device,
        }
    }

    /// Milliseconds since the epoch, as the writing device read its clock (or a later one it
    /// had seen).
    pub fn wall(&self) -> u64 {
        self.wall
    }

    /// The device that made it.
    pub fn device(&self) -> &DeviceId {
        &self.device
    }

    /// `<wall hex 16>-<counter hex 8>-<device>`: sorts as the stamp does.
    pub fn text(&self) -> String {
        format!("{:016x}-{:08x}-{}", self.wall, self.counter, self.device)
    }

    /// The stamp `text` names, if it names one.
    pub fn parse(text: &str) -> Option<Self> {
        let (wall, rest) = text.split_once('-')?;
        let (counter, device) = rest.split_once('-')?;
        (wall.len() == 16 && counter.len() == 8).then_some(())?;
        Some(Self {
            wall: u64::from_str_radix(wall, 16).ok()?,
            counter: u32::from_str_radix(counter, 16).ok()?,
            device: DeviceId::parse(device)?,
        })
    }
}

impl Serialize for Hlc {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text())
    }
}

impl<'de> Deserialize<'de> for Hlc {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Hlc::parse(&text).ok_or_else(|| serde::de::Error::custom("not a clock stamp"))
    }
}

/// The wall clock, in milliseconds, as a seam.
pub trait Millis: std::fmt::Debug + Send + Sync {
    /// Now.
    fn now_ms(&self) -> u64;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemMillis;

impl Millis for SystemMillis {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

/// A clock a test sets.
#[derive(Debug, Default)]
pub struct ManualMillis(AtomicU64);

impl ManualMillis {
    /// A clock at `ms`.
    pub fn at(ms: u64) -> Self {
        Self(AtomicU64::new(ms))
    }

    /// Moves it to `ms` (it may go backwards: a device's clock can).
    pub fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::Relaxed);
    }
}

impl Millis for ManualMillis {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// The generator of one device's stamps: strictly increasing, and later than every stamp it was
/// told it saw, whatever its wall clock does.
#[derive(Debug, Clone)]
pub struct HlcClock {
    last: Hlc,
}

impl HlcClock {
    /// A clock for `device` that has made and seen nothing.
    pub fn new(device: DeviceId) -> Self {
        Self {
            last: Hlc::new(0, 0, device),
        }
    }

    /// Notes a stamp from anywhere: the next one is later than it.
    pub fn observe(&mut self, seen: &Hlc) {
        if (seen.wall, seen.counter) > (self.last.wall, self.last.counter) {
            self.last.wall = seen.wall;
            self.last.counter = seen.counter;
        }
    }

    /// The next stamp, the wall clock reading `now_ms`.
    pub fn tick(&mut self, now_ms: u64) -> Hlc {
        if now_ms > self.last.wall {
            self.last.wall = now_ms;
            self.last.counter = 0;
        } else {
            self.last.counter = self.last.counter.saturating_add(1);
        }
        self.last.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str) -> DeviceId {
        DeviceId::parse(name).expect("device")
    }

    #[test]
    fn a_device_name_is_a_short_plain_word() {
        const CASES: &[(&str, bool)] = &[
            ("a", true),
            ("laptop2", true),
            ("0123456789abcdef", true),
            ("", false),
            ("0123456789abcdefg", false),
            ("Laptop", false),
            ("a-b", false),
            ("a/b", false),
            ("a.b", false),
        ];
        for (name, ok) in CASES {
            assert_eq!(DeviceId::parse(name).is_some(), *ok, "{name:?}");
        }
    }

    #[test]
    fn stamps_order_by_wall_then_counter_then_device_and_their_text_sorts_the_same() {
        let stamps = [
            Hlc::new(1, 0, device("a")),
            Hlc::new(1, 0, device("b")),
            Hlc::new(1, 1, device("a")),
            Hlc::new(2, 0, device("a")),
            Hlc::new(0x1_0000_0000_0000, 0, device("a")),
        ];
        for pair in stamps.windows(2) {
            assert!(pair[0] < pair[1], "{pair:?}");
            assert!(pair[0].text() < pair[1].text(), "{pair:?}");
        }
    }

    #[test]
    fn a_stamp_round_trips_through_its_text_and_json_and_bad_text_is_refused() {
        let stamp = Hlc::new(1_760_000_000_123, 7, device("laptop"));
        assert_eq!(
            stamp.text(),
            format!("{:016x}-00000007-laptop", 1_760_000_000_123u64)
        );
        assert_eq!(Hlc::parse(&stamp.text()), Some(stamp.clone()));
        let json = serde_json::to_string(&stamp).expect("json");
        assert_eq!(serde_json::from_str::<Hlc>(&json).expect("stamp"), stamp);
        for bad in [
            "",
            "1-2-a",
            "zzzzzzzzzzzzzzzz-00000000-a",
            "0000000000000001-00000000-",
        ] {
            assert_eq!(Hlc::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_clock_never_repeats_or_goes_back_even_when_the_wall_clock_does() {
        let mut clock = HlcClock::new(device("a"));
        let first = clock.tick(1_000);
        let same_ms = clock.tick(1_000);
        let back = clock.tick(500);
        let later = clock.tick(2_000);
        assert!(first < same_ms && same_ms < back && back < later);
        assert_eq!((back.wall(), later.wall()), (1_000, 2_000));
    }

    #[test]
    fn a_stamp_seen_from_another_device_puts_the_next_one_after_it() {
        let mut clock = HlcClock::new(device("a"));
        let theirs = Hlc::new(9_000, 3, device("b"));
        clock.observe(&theirs);
        let next = clock.tick(1_000);
        assert!(next > theirs, "{next:?}");
        assert_eq!(next.device(), &device("a"));
        // An older stamp changes nothing.
        clock.observe(&Hlc::new(1, 0, device("b")));
        assert!(clock.tick(1_000) > next);
    }
}

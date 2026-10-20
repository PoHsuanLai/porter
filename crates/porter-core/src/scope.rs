//! The name of the systemd scope a launcher starts an app in: `app-[<launcher>-]<app>-<run>.scope`
//! (XDG "Desktop Entry / systemd integration"). sill writes one for each launch, Flatpak writes
//! one for each run, and porter reads them to learn which app a process is (`identity_of`), so the
//! writer and the reader are one type, [`ScopeName`], and what one writes the other reads back.
//!
//! The app id is escaped as a unit name component, the way `systemd-escape` does: every byte
//! outside `[A-Za-z0-9:_.]`, and a leading `.`, becomes `\xHH`. A literal `-` in an id is
//! therefore `\x2d`, and the dashes that are left split the name without ambiguity. The launcher
//! tag and the run are each one plain word of ASCII letters and digits.
//!
//! ```
//! use porter_core::ScopeName;
//!
//! let written = ScopeName::new(Some("sill"), "com.example.my-app", "4").expect("plain words");
//! assert_eq!(written.to_string(), r"app-sill-com.example.my\x2dapp-4.scope");
//! let read = ScopeName::parse(&written.to_string()).expect("a scope name");
//! assert_eq!(read, written);
//! assert_eq!(read.app(), "com.example.my-app");
//! ```

use crate::error::CoreError;
use std::fmt;

/// The launcher tag Flatpak writes into its own scopes' names.
pub const FLATPAK_LAUNCHER: &str = "flatpak";

/// One app scope's unit name, parsed: who launched it, which app, and which run of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeName {
    launcher: Option<String>,
    app: String,
    run: String,
}

impl ScopeName {
    /// The scope `launcher` (if any) starts for `run` of `app`. `app` is the id as written in
    /// its desktop entry, unescaped; `launcher` and `run` must each be one plain word.
    pub fn new(launcher: Option<&str>, app: &str, run: &str) -> Result<ScopeName, CoreError> {
        let malformed = |what, text: &str| CoreError::MalformedId {
            what,
            text: text.to_owned(),
        };
        if app.is_empty() {
            return Err(malformed("scope app", app));
        }
        if let Some(launcher) = launcher.filter(|launcher| !is_word(launcher)) {
            return Err(malformed("scope launcher", launcher));
        }
        if !is_word(run) {
            return Err(malformed("scope run", run));
        }
        Ok(ScopeName {
            launcher: launcher.map(str::to_owned),
            app: app.to_owned(),
            run: run.to_owned(),
        })
    }

    /// Read a unit name back, or `None` for one that is not an app scope's: no `app-` or
    /// `.scope`, too many or too few parts, a launcher or run that is not one word, or a broken
    /// escape in the id.
    pub fn parse(unit: &str) -> Option<ScopeName> {
        let body = unit.strip_prefix("app-")?.strip_suffix(".scope")?;
        let (launcher, id, run) = match body.split('-').collect::<Vec<_>>().as_slice() {
            [id, run] => (None, *id, *run),
            [launcher, id, run] if is_word(launcher) => (Some(*launcher), *id, *run),
            _ => return None,
        };
        let app = unescape(id)?;
        ScopeName::new(launcher, &app, run).ok()
    }

    /// The tag of the launcher that wrote it (`sill`, `flatpak`), if it wrote one.
    pub fn launcher(&self) -> Option<&str> {
        self.launcher.as_deref()
    }

    /// The app's id, unescaped.
    pub fn app(&self) -> &str {
        &self.app
    }

    /// Which run of the app: the launcher's nonce, or Flatpak's instance number.
    pub fn run(&self) -> &str {
        &self.run
    }

    /// Whether Flatpak wrote it: its tag, and a decimal instance number, which is what Flatpak
    /// writes for each run.
    pub fn is_flatpak(&self) -> bool {
        self.launcher() == Some(FLATPAK_LAUNCHER) && self.run.bytes().all(|b| b.is_ascii_digit())
    }
}

impl fmt::Display for ScopeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("app-")?;
        if let Some(launcher) = &self.launcher {
            write!(f, "{launcher}-")?;
        }
        write!(f, "{}-{}.scope", escape(&self.app), self.run)
    }
}

/// Whether `text` is one non-empty run of ASCII letters and digits.
fn is_word(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// `text` as a unit name component: `\xHH` for each byte outside `[A-Za-z0-9:_.]` and for a
/// leading `.`. Unlike `systemd-escape` a `/` is escaped too rather than written `-`, because
/// this is a name and not a path, and a `-` here would split it.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, byte) in text.bytes().enumerate() {
        let plain = byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'.');
        if plain && !(i == 0 && byte == b'.') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    }
    out
}

/// A unit name component with its `\xHH` escapes undone, or `None` for a broken escape.
fn unescape(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        rest = match (first, tail) {
            (b'\\', [b'x', high, low, after @ ..]) => {
                bytes.push((hex_digit(*high)? << 4) | hex_digit(*low)?);
                after
            }
            (b'\\', _) => return None,
            _ => {
                bytes.push(first);
                tail
            }
        };
    }
    String::from_utf8(bytes).ok()
}

/// The value of one hexadecimal digit.
fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

#[cfg(test)]
mod tests;

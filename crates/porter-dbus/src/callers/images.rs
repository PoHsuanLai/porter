//! Where the desktop installs its programs, for the rows of a caller table that name a unit and
//! no program. The patterns are peephole's `exe` patterns: absolute, at most one `*` in a segment.

/// The kind of place a program of the desktop is installed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    /// A daemon, started by a unit or by D-Bus activation: `libexec/quire/<name>`.
    Libexec,
    /// A program a person may also start: `bin/<name>`.
    Bin,
}

/// The roots a package (`/usr`), a local install (`/usr/local`) and a release bundle
/// (`/opt/quire/releases/<version>`) put the desktop under. Every one is root-owned; a home
/// directory is not among them.
const ROOTS: [&str; 3] = ["/usr", "/usr/local", "/opt/quire/releases/*"];

/// The images of the program `name` installed as `install` under each of the roots.
pub fn installed(install: Install, name: &str) -> Vec<String> {
    let folder = match install {
        Install::Libexec => "libexec/quire",
        Install::Bin => "bin",
    };
    ROOTS
        .iter()
        .map(|root| format!("{root}/{folder}/{name}"))
        .collect()
}

/// The daemon a unit named `<name>.service` is, when `<name>` is lower case letters, digits and
/// dashes (`inferd`, `docket-acp`); a dotted unit (`org.quire.Settings.service`) is an app that
/// D-Bus activates, whose program the unit's name does not give.
pub(super) fn daemon_name(unit: &str) -> Option<&str> {
    let name = unit.strip_suffix(".service")?;
    let plain = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
    (!name.is_empty() && name.bytes().all(plain)).then_some(name)
}

#[cfg(test)]
mod tests;

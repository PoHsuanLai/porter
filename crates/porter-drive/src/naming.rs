//! The names a drive's items are shown by: the one place both naming rules live.
//!
//! - A name with `/` (Drive allows it) is shown with `／` (U+FF0F, the fullwidth solidus) in its
//!   place, as rclone shows it.
//! - Of the items one folder holds under one name (Drive allows that too), the oldest by id keeps
//!   it and the others are "Name 2.ext", "Name 3.ext", in id order, each the first number no other
//!   item there is shown by: the rule Save a Copy names its copies by. Items made on this computer
//!   come after the drive's own.
//!
//! The names depend only on what the folder holds, so the same listing is shown by the same names
//! after a restart.
//!
//! Back the other way, a name given here ([`drive_name`]):
//! - A name made here (a new file or folder, or a rename to a new name) is sent as written:
//!   a `／` typed here is a `／` on the drive too.
//! - A name left as it is shown (a move to another folder under the same name) keeps the
//!   drive's own name: the `／` that stands for a `/`, and a " 2" these rules added, are never
//!   sent.
//! - An online document's link file renamed keeps its link extension out of the drive's name:
//!   "Plan.gdoc" renamed "Budget.gdoc" is the document "Budget".

use porter_sync::ItemKind;
use std::collections::BTreeSet;

/// What a `/` in a name is shown as.
pub const SLASH: char = '\u{FF0F}';

/// What marks this computer's version of a file kept beside the drive's ([`marked`]).
pub const THIS_COMPUTER: &str = " (this computer)";

/// The MIME types Google's documents are saved as, and the extension each saved copy and each
/// link file is named with.
const KINDS: &[(&str, &str, &str)] = &[
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docx",
        "gdoc",
    ),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsx",
        "gsheet",
    ),
    (
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pptx",
        "gslides",
    ),
    ("image/png", "png", "glink"),
];

/// The extension of the link file of a document saved as `export`: `gdoc`, `gsheet`,
/// `gslides`, or `glink` for every other kind and for shortcuts.
pub fn link_extension(export: Option<&str>) -> &'static str {
    export
        .and_then(|mime| KINDS.iter().find(|(m, _, _)| *m == mime))
        .map_or("glink", |(_, _, link)| link)
}

/// The extension a copy saved as `export` is named with; `None` for a type with no file kind
/// here.
pub fn copy_extension(export: &str) -> Option<&'static str> {
    KINDS
        .iter()
        .find(|(mime, _, _)| *mime == export)
        .map(|(_, ext, _)| *ext)
}

/// `name` with each `/` shown as [`SLASH`]; a name a folder cannot hold (`""`, `.`, `..`) is
/// shown with the slash in it too, so it can.
pub fn shown_base(name: &str) -> String {
    match name {
        "" | "." | ".." => format!("{name}{SLASH}"),
        _ => name.replace('/', &SLASH.to_string()),
    }
}

/// The name an item of `kind` named `name` is shown by before two of a folder are told apart:
/// an online document's has its link file's extension.
pub fn base_of(name: &str, kind: &ItemKind) -> String {
    let base = shown_base(name);
    match kind {
        ItemKind::Online { export, .. } => {
            format!("{base}.{}", link_extension(export.as_deref()))
        }
        _ => base,
    }
}

/// The stem and extension of `name`, split at its last dot when both sides have something.
fn split(name: &str) -> Option<(&str, &str)> {
    name.rsplit_once('.')
        .filter(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
}

/// `name` numbered `n`: "Plan.docx" as "Plan 2.docx", "Notes" as "Notes 2".
pub fn numbered(name: &str, n: u32) -> String {
    match split(name) {
        Some((stem, ext)) => format!("{stem} {n}.{ext}"),
        None => format!("{name} {n}"),
    }
}

/// `name` with `mark` before its extension: "Plan.docx" marked " (this computer)" is
/// "Plan (this computer).docx".
pub fn marked(name: &str, mark: &str) -> String {
    match split(name) {
        Some((stem, ext)) => format!("{stem}{mark}.{ext}"),
        None => format!("{name}{mark}"),
    }
}

/// The name the drive is to have for an item of `kind` shown as `shown` and now named `drive`
/// on the drive, when it is given the shown name `wanted` here (see the module's rules).
pub fn drive_name(wanted: &str, shown: &str, drive: &str, kind: &ItemKind) -> String {
    if wanted == shown {
        return drive.to_owned();
    }
    match kind {
        ItemKind::Online { export, .. } => {
            let extension = format!(".{}", link_extension(export.as_deref()));
            wanted
                .strip_suffix(&extension)
                .filter(|stem| !stem.is_empty())
                .unwrap_or(wanted)
                .to_owned()
        }
        _ => wanted.to_owned(),
    }
}

/// The first of `name`, "Name 2.ext", "Name 3.ext", ... that `taken` does not hold.
pub fn free(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    (2..)
        .map(|n| numbered(name, n))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_owned())
}

/// The shown name of each of one folder's items, given in order oldest first (the drive's by id,
/// then what this computer made) with each one's [`base_of`] name: the first of a name keeps it,
/// the rest are numbered. Answers in the same order.
pub fn assign(bases: &[String]) -> Vec<String> {
    let mut taken: BTreeSet<&str> = BTreeSet::new();
    let mut first = vec![false; bases.len()];
    for (i, base) in bases.iter().enumerate() {
        first[i] = taken.insert(base.as_str());
    }
    let mut taken: BTreeSet<String> = taken.into_iter().map(str::to_owned).collect();
    bases
        .iter()
        .zip(first)
        .map(|(base, first)| match first {
            true => base.clone(),
            false => {
                let name = free(base, |candidate| taken.contains(candidate));
                taken.insert(name.clone());
                name
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_ids::WebUrl;

    fn names(bases: &[&str]) -> Vec<String> {
        assign(&bases.iter().map(|b| (*b).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn a_slash_is_shown_as_the_fullwidth_solidus_and_an_empty_name_still_names() {
        assert_eq!(shown_base("2025/26 budget.xlsx"), "2025／26 budget.xlsx");
        assert_eq!(shown_base(".."), "..／");
        assert_eq!(shown_base(""), "／");
    }

    #[test]
    fn the_first_of_a_name_keeps_it_and_the_rest_are_numbered_around_what_is_there() {
        assert_eq!(
            names(&[
                "Plan.docx",
                "Plan.docx",
                "Plan 2.docx",
                "Plan.docx",
                "Notes",
                "Notes"
            ]),
            [
                "Plan.docx",
                "Plan 3.docx",
                "Plan 2.docx",
                "Plan 4.docx",
                "Notes",
                "Notes 2"
            ]
        );
        assert_eq!(numbered(".bashrc", 2), ".bashrc 2");
    }

    #[test]
    fn a_name_given_here_is_sent_as_written_and_one_left_as_shown_keeps_the_drives() {
        let file = ItemKind::File;
        assert_eq!(drive_name("a／b", "x", "x", &file), "a／b");
        assert_eq!(drive_name("a／b", "a／b", "a/b", &file), "a/b");
        assert_eq!(
            drive_name("Plan 2.docx", "Plan 2.docx", "Plan.docx", &file),
            "Plan.docx"
        );
        let url = WebUrl::parse("https://docs.example.test/d/1").expect("url");
        let doc = ItemKind::Online {
            url,
            export: Some(KINDS[0].0.to_owned()),
        };
        assert_eq!(
            drive_name("Budget.gdoc", "Plan.gdoc", "Plan", &doc),
            "Budget"
        );
        assert_eq!(drive_name("Budget", "Plan.gdoc", "Plan", &doc), "Budget");
        assert_eq!(
            marked("Plan.docx", " (this computer)"),
            "Plan (this computer).docx"
        );
        assert_eq!(marked("Notes", " (this computer)"), "Notes (this computer)");
    }

    #[test]
    fn an_online_document_is_named_by_what_it_saves_as() {
        let url = WebUrl::parse("https://docs.example.test/d/1").expect("url");
        let online = |export: Option<&str>| ItemKind::Online {
            url: url.clone(),
            export: export.map(str::to_owned),
        };
        assert_eq!(
            base_of("Plan", &online(Some(KINDS[0].0))),
            "Plan.gdoc".to_owned()
        );
        assert_eq!(base_of("Sums", &online(Some(KINDS[1].0))), "Sums.gsheet");
        assert_eq!(base_of("Talk", &online(Some(KINDS[2].0))), "Talk.gslides");
        assert_eq!(base_of("Survey", &online(None)), "Survey.glink");
        assert_eq!(base_of("a/b", &ItemKind::File), "a／b");
        assert_eq!(copy_extension(KINDS[0].0), Some("docx"));
        assert_eq!(copy_extension("application/pdf"), None);
    }
}

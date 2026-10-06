//! Collection names as the mirror keeps them: the directory under the account's vdir and the
//! dataset slug (`Sync1` names it `<account>/<slug>`, and its journal is `<slug>.sqlite`).

use super::PimKind;
use super::discover::Found;
use crate::dataset::DatasetId;
use sha2::{Digest, Sha256};

/// A discovered collection with the names it is kept under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// What the server said.
    pub found: Found,
    /// Its directory under `<vdir>/<account>/`.
    pub dir: String,
    /// Its dataset.
    pub dataset: DatasetId,
}

fn tag(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(3)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A directory name: the URL segment with everything but letters, digits and `._-` made `_`
/// and no leading dot; never empty.
fn dir_name(segment: &str) -> String {
    let cleaned: String = segment
        .chars()
        .map(|c| match c.is_ascii_alphanumeric() || "._-".contains(c) {
            true => c,
            false => '_',
        })
        .collect();
    match cleaned.trim_start_matches('.') {
        "" => "collection".to_owned(),
        name => name.to_owned(),
    }
}

/// The slug of a dataset: the kind's prefix and the directory name lowercased to `[a-z0-9_]`,
/// cut to fit the 48 characters a dataset name may have.
fn slug(kind: PimKind, dir: &str) -> String {
    let body: String = dir
        .chars()
        .map(|c| match c.is_ascii_alphanumeric() {
            true => c.to_ascii_lowercase(),
            false => '_',
        })
        .collect();
    let room = 48 - kind.slug().len() - 1;
    match body.len() > room {
        true => format!("{}_{}_{}", kind.slug(), &body[..room - 7], tag(dir)),
        false => format!("{}_{body}", kind.slug()),
    }
}

/// Names for `found`, in order. A calendar is its segment and an address book its segment plus
/// `-contacts`, always, so a calendar and an address book of one account never share a
/// directory whatever order they are found in. Two of one kind that would share a directory (or
/// a slug, which is only lowercasing of it) all get the same name plus a short hash of their
/// URL, so the names depend on the set and never on the order the server lists it in.
pub fn plan(kind: PimKind, found: Vec<Found>) -> Vec<Planned> {
    let names: Vec<(String, String)> = found
        .iter()
        .map(|f| {
            let dir = format!("{}{}", dir_name(&f.segment), kind.dir_suffix());
            let slug = slug(kind, &dir);
            (dir, slug)
        })
        .collect();
    let clashes = |at: usize| {
        names
            .iter()
            .enumerate()
            .any(|(other, n)| other != at && (n.0 == names[at].0 || n.1 == names[at].1))
    };
    found
        .into_iter()
        .enumerate()
        .map(|(at, found)| {
            let (dir, slug) = match clashes(at) {
                false => names[at].clone(),
                true => {
                    let dir = format!("{}-{}", names[at].0, tag(found.url.as_str()));
                    let slug = slug(kind, &dir);
                    (dir, slug)
                }
            };
            Planned {
                dataset: DatasetId::parse(&slug).expect("a slug made of [a-z0-9_]"),
                dir,
                found,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::WebUrl;

    fn found(segment: &str, url: &str) -> Found {
        Found {
            url: WebUrl::parse(url).expect("url"),
            segment: segment.to_owned(),
            displayname: None,
            color: None,
        }
    }

    fn names(kind: PimKind, items: &[(&str, &str)]) -> Vec<(String, String)> {
        plan(kind, items.iter().map(|(s, u)| found(s, u)).collect())
            .into_iter()
            .map(|p| (p.dir, p.dataset.to_string()))
            .collect()
    }

    #[test]
    fn a_collection_is_its_url_segment_made_safe() {
        const CASES: &[(&str, &str, &str)] = &[
            ("personal", "personal", "pim_cal_personal"),
            ("Work Stuff", "Work_Stuff", "pim_cal_work_stuff"),
            ("../evil", "_evil", "pim_cal__evil"),
            (".hidden", "hidden", "pim_cal_hidden"),
            ("..", "collection", "pim_cal_collection"),
            (
                "contact_birthdays",
                "contact_birthdays",
                "pim_cal_contact_birthdays",
            ),
            ("a-b.c", "a-b.c", "pim_cal_a_b_c"),
        ];
        for (segment, dir, dataset) in CASES {
            let got = names(PimKind::Calendar, &[(segment, "http://127.0.0.1:1/x/")]);
            assert_eq!(
                got,
                [((*dir).to_owned(), (*dataset).to_owned())],
                "{segment}"
            );
        }
    }

    #[test]
    fn a_long_name_is_cut_to_a_slug_that_still_names_one_collection() {
        let long = "x".repeat(80);
        let other = format!("{}y", "x".repeat(80));
        let got = names(
            PimKind::Contacts,
            &[
                (&long, "http://127.0.0.1:1/1/"),
                (&other, "http://127.0.0.1:1/2/"),
            ],
        );
        for (_, slug) in &got {
            assert!(slug.len() <= 48, "{slug}");
            assert!(slug.starts_with("pim_card_"));
        }
        assert_ne!(got[0].1, got[1].1);
    }

    #[test]
    fn names_that_clash_all_take_a_hash_and_do_not_depend_on_the_order() {
        let a = ("Work", "http://127.0.0.1:1/a/");
        let b = ("work", "http://127.0.0.1:1/b/");
        let c = ("home", "http://127.0.0.1:1/c/");
        let one = names(PimKind::Calendar, &[a, b, c]);
        let two = names(PimKind::Calendar, &[c, b, a]);
        assert_eq!(one[2].0, "home", "an unclashed one keeps its name");
        assert!(one[0].0.starts_with("Work-") && one[1].0.starts_with("work-"));
        assert_ne!(one[0], one[1]);
        let mut sorted_one = one.clone();
        let mut sorted_two = two.clone();
        sorted_one.sort();
        sorted_two.sort();
        assert_eq!(sorted_one, sorted_two);
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;
    use porter_core::WebUrl;

    #[test]
    fn a_calendar_and_an_address_book_of_one_name_never_share_a_directory_or_a_dataset() {
        let one = |kind| {
            plan(
                kind,
                vec![Found {
                    url: WebUrl::parse("http://127.0.0.1:1/shared/").expect("url"),
                    segment: "shared".to_owned(),
                    displayname: None,
                    color: None,
                }],
            )
            .remove(0)
        };
        let (calendar, book) = (one(PimKind::Calendar), one(PimKind::Contacts));
        assert_eq!(
            (calendar.dir.as_str(), book.dir.as_str()),
            ("shared", "shared-contacts")
        );
        assert_ne!(calendar.dataset, book.dataset);
    }
}

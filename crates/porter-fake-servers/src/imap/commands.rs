//! The commands of a logged-in IMAP session (SELECT, FETCH) and the command-line syntax.

use super::Shared;
use crate::mail::Lines;
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};

pub(super) async fn select<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    shared: &Shared,
    tag: &str,
    verb: &str,
) -> io::Result<()> {
    let count = shared.inbox.len();
    lines.send(&format!("* {count} EXISTS")).await?;
    lines.send("* 0 RECENT").await?;
    lines
        .send("* FLAGS (\\Seen \\Answered \\Flagged \\Deleted \\Draft)")
        .await?;
    lines.send("* OK [UIDVALIDITY 1] uids valid").await?;
    lines
        .send(&format!("* OK [UIDNEXT {}] next uid", count + 1))
        .await?;
    let mode = if verb == "EXAMINE" {
        "READ-ONLY"
    } else {
        "READ-WRITE"
    };
    lines
        .send(&format!("{tag} OK [{mode}] {verb} completed"))
        .await
}

/// The numbers a set like `1`, `2:4` or `1:*` names, among `1..=max`.
fn expand(set: &str, max: u32) -> Vec<u32> {
    let bound = |s: &str| if s == "*" { Some(max) } else { s.parse().ok() };
    set.split(',')
        .flat_map(|part| match part.split_once(':') {
            Some((a, b)) => match (bound(a), bound(b)) {
                (Some(a), Some(b)) => (a.min(b)..=a.max(b)).collect(),
                _ => Vec::new(),
            },
            None => bound(part).into_iter().collect(),
        })
        .filter(|n| (1..=max).contains(n))
        .collect()
}

pub(super) async fn fetch<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    shared: &Shared,
    tag: &str,
    verb: &str,
    rest: &[String],
) -> io::Result<()> {
    let by_uid = verb == "UID";
    let rest: &[String] = if by_uid {
        match rest.first().map(|v| v.to_ascii_uppercase()).as_deref() {
            Some("FETCH") => &rest[1..],
            _ => {
                return lines
                    .send(&format!("{tag} BAD only UID FETCH is supported"))
                    .await;
            }
        }
    } else {
        rest
    };
    let (Some(set), items) = (
        rest.first(),
        rest[rest.len().min(1)..].join(" ").to_ascii_uppercase(),
    ) else {
        return lines
            .send(&format!("{tag} BAD FETCH needs a set and items"))
            .await;
    };
    // UIDs run 1..n here, so a UID set and a sequence set name the same messages.
    let max = u32::try_from(shared.inbox.len()).unwrap_or(u32::MAX);
    for number in expand(set, max) {
        let message = &shared.inbox[(number - 1) as usize];
        let mut items_out = vec![format!("UID {}", message.uid)];
        if items.contains("FLAGS") || items.contains("ALL") {
            items_out.push("FLAGS ()".to_owned());
        }
        if items.contains("RFC822.SIZE") {
            items_out.push(format!("RFC822.SIZE {}", message.raw.len()));
        }
        let wants_body = items.contains("BODY[")
            || items.contains("BODY.PEEK[")
            || items.contains("RFC822)")
            || items.ends_with("RFC822");
        let head = format!("* {number} FETCH ({}", items_out.join(" "));
        match wants_body {
            true => {
                lines
                    .send_raw(&format!(
                        "{head} BODY[] {{{}}}\r\n{})\r\n",
                        message.raw.len(),
                        message.raw
                    ))
                    .await?;
            }
            false => lines.send(&format!("{head})")).await?,
        }
    }
    lines.send(&format!("{tag} OK FETCH completed")).await
}

/// Splits a command into words: atoms, quoted strings and literals (`{n}` / `{n+}`), reading the
/// literal bytes from the stream.
pub(super) async fn read_args<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    first: &str,
) -> io::Result<Vec<String>> {
    let mut words = Vec::new();
    let mut line = first.to_owned();
    loop {
        let (mut more, literal) = tokenize(&line);
        words.append(&mut more);
        let Some((len, synchronizing)) = literal else {
            return Ok(words);
        };
        if synchronizing {
            lines.send("+ go ahead").await?;
        }
        let bytes = lines.read_exact(len).await?;
        words.push(String::from_utf8_lossy(&bytes).into_owned());
        line = lines.read_line().await?.unwrap_or_default();
    }
}

fn tokenize(line: &str) -> (Vec<String>, Option<(usize, bool)>) {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' => {
                chars.next();
            }
            '"' => {
                chars.next();
                let mut word = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => word.extend(chars.next()),
                        '"' => break,
                        c => word.push(c),
                    }
                }
                words.push(word);
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ' ' {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                let literal = word
                    .strip_prefix('{')
                    .and_then(|w| w.strip_suffix('}'))
                    .map(|w| (w.trim_end_matches('+'), w.ends_with('+')))
                    .and_then(|(digits, plus)| digits.parse::<usize>().ok().map(|n| (n, !plus)));
                match (literal, chars.peek()) {
                    (Some(literal), None) => return (words, Some(literal)),
                    _ => words.push(word),
                }
            }
        }
    }
    (words, None)
}

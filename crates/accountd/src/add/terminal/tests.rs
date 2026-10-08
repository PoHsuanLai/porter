use super::*;
use porter_core::ProviderId;
use porter_core::sheet::{Protocol, SignInView, manual_form};
use std::collections::VecDeque;

/// A terminal that answers from a script and records what it was asked.
#[derive(Debug, Default)]
struct Script {
    answers: Mutex<VecDeque<String>>,
    asked: Mutex<Vec<(String, Echo)>>,
}

impl Terminal for Script {
    fn say(&self, _line: &str) {}

    fn ask(&self, label: &str, echo: Echo) -> Option<String> {
        self.asked
            .lock()
            .expect("asked")
            .push((label.to_owned(), echo));
        self.answers.lock().expect("answers").pop_front()
    }

    fn open_browser(&self, _url: &str) {}
}

fn link(answers: &[&str]) -> TerminalLink<Script> {
    let script = Script {
        answers: Mutex::new(answers.iter().map(|a| (*a).to_owned()).collect()),
        ..Script::default()
    };
    TerminalLink {
        inner: Arc::new(Inner {
            terminal: script,
            allowing: Mutex::new(None),
        }),
        view: None,
    }
}

fn form() -> SignInView {
    SignInView {
        row: None,
        provider: ProviderId::parse("generic-imap").expect("id"),
        fields: manual_form(Protocol::Imap, Some("example.org")),
        problem: None,
    }
}

fn submitted(input: SheetInput) -> Vec<(FieldKind, String)> {
    let SheetInput::Submit(answers) = input else {
        panic!("{input:?}")
    };
    answers
        .into_iter()
        .map(|a| {
            let text = match a.value {
                FieldValue::Plain(t) => t,
                FieldValue::Secret(s) => s.expose().to_owned(),
            };
            (a.kind, text)
        })
        .collect()
}

fn asked(link: &TerminalLink<Script>) -> Vec<String> {
    let asked = link.inner.terminal.asked.lock().expect("asked");
    asked.iter().map(|(label, _)| label.clone()).collect()
}

#[tokio::test]
async fn an_empty_answer_takes_the_default_and_a_choice_names_its_values() {
    // imap, host, starttls, the port left empty, smtp host, starttls, port empty, no login.
    let link = link(&[
        "",
        "mail.example.org",
        "starttls",
        "",
        "",
        "starttls",
        "",
        "",
    ]);
    let answers = submitted(link.fields(form()).await.expect("answers"));
    use FieldKind::*;
    assert_eq!(
        answers,
        [
            (Protocol, "imap".to_owned()),
            (Server, "mail.example.org".to_owned()),
            (Security, "starttls".to_owned()),
            (Port, "143".to_owned()),
            (OutgoingServer, "smtp.example.org".to_owned()),
            (OutgoingSecurity, "starttls".to_owned()),
            (OutgoingPort, "587".to_owned()),
            (Username, String::new()),
        ],
        "an empty host is the guess, an empty port is the one the security uses"
    );
    let asked = asked(&link);
    assert_eq!(asked[0], "Mail protocol [imap, pop3 or jmap] (empty: imap)");
    assert_eq!(asked[1], "Server (empty: imap.example.org)");
    assert_eq!(asked[2], "Security [tls or starttls] (empty: tls)");
    assert_eq!(asked[3], "Port (optional) (empty: 143)");
    assert_eq!(asked[7], "User name (optional)");
}

#[tokio::test]
async fn jmap_asks_for_the_session_and_a_hidden_token_and_no_outgoing_server() {
    let link = link(&["jmap", "", "tok-1", "ada"]);
    let answers = submitted(link.fields(form()).await.expect("answers"));
    use FieldKind::*;
    assert_eq!(
        answers,
        [
            (Protocol, "jmap".to_owned()),
            (
                SessionUrl,
                "https://example.org/.well-known/jmap".to_owned()
            ),
            (Token, "tok-1".to_owned()),
            (Username, "ada".to_owned()),
        ]
    );
    let terminal = &link.inner.terminal;
    let asked = terminal.asked.lock().expect("asked");
    assert_eq!(asked[2].0, "API token (hidden) (optional)");
    assert_eq!(asked[2].1, Echo::Off, "a token is typed hidden");
}

#[tokio::test]
async fn pop3_prefills_its_own_guess_and_ports() {
    let link = link(&["pop3", "", "", "", "", "", "", ""]);
    let answers = submitted(link.fields(form()).await.expect("answers"));
    use FieldKind::*;
    let of = |kind| {
        answers
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, t)| t.as_str())
    };
    assert_eq!(of(Server), Some("pop.example.org"));
    assert_eq!(of(Port), Some("995"));
    assert_eq!(of(OutgoingPort), Some("465"));
}

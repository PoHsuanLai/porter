use super::*;
use porter_core::ProviderId;
use porter_sheet::{Protocol, SignInView, manual_form};
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

#[tokio::test]
async fn an_empty_answer_takes_the_default_and_a_hidden_field_is_typed_hidden() {
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
    let asked = link.inner.terminal.asked.lock().expect("asked");
    assert_eq!(asked[2].0, "API token (hidden) (optional)");
    assert_eq!(asked[2].1, Echo::Off, "a token is typed hidden");
}

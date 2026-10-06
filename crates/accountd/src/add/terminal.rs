//! The sheet of `accountd add`, drawn on a terminal. It implements the service's `Sheets` seam:
//! a conversation prints what the sheet would show and answers with what the person types, so
//! the same sign-in the sheet host drives runs from a shell, and no key or password goes
//! anywhere but into the sign-in.
//!
//! The terminal itself is a seam (`Terminal`): standard input and output in the binary, a script
//! in tests.

use porter_core::consent::{ConsentAnswer, ConsentAsk, GrantScope};
use porter_core::sheet::{
    Entry, FieldAnswer, FieldKind, FieldValue, Presence, ServiceChoice, ServiceState, SheetInput,
    SheetView,
};
use porter_core::wire::ParentWindow;
use porter_core::{AccountId, SecretText, Toggle};
use porter_service::{SheetFault, SheetLink, SheetOpen, Sheets};
use std::io::{BufRead, IsTerminal, Write};
use std::sync::{Arc, Mutex, PoisonError};

/// Whether what is typed is echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Echo {
    /// Shown as typed.
    On,
    /// Hidden: a key or a password.
    Off,
}

/// The person's terminal. Its calls block; the sheet runs them off the async threads.
pub trait Terminal: Send + Sync + 'static {
    /// Shows one line.
    fn say(&self, line: &str);

    /// Shows `label` and reads one line, or `None` when the input has ended.
    fn ask(&self, label: &str, echo: Echo) -> Option<String>;

    /// Opens `url` in the person's browser, as best it can; the url has been shown already.
    fn open_browser(&self, url: &str);
}

/// Standard input and output. A hidden entry turns the terminal's echo off with `stty` for the
/// line and restores it after, and only when standard input is a terminal.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdTerminal;

/// Restores the terminal's echo when dropped.
struct EchoOff;

impl EchoOff {
    fn new() -> Option<Self> {
        std::io::stdin()
            .is_terminal()
            .then(|| stty("-echo"))
            .flatten()
            .map(|()| Self)
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        let _ = stty("echo");
    }
}

fn stty(arg: &str) -> Option<()> {
    std::process::Command::new("stty")
        .arg(arg)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .filter(std::process::ExitStatus::success)
        .map(|_| ())
}

impl Terminal for StdTerminal {
    fn say(&self, line: &str) {
        println!("{line}");
    }

    fn ask(&self, label: &str, echo: Echo) -> Option<String> {
        let _hidden = (echo == Echo::Off).then(EchoOff::new).flatten();
        eprint!("{label}: ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        let read = std::io::stdin().lock().read_line(&mut line).ok()?;
        if echo == Echo::Off {
            eprintln!();
        }
        (read > 0).then(|| line.trim_end_matches(['\r', '\n']).to_owned())
    }

    fn open_browser(&self, url: &str) {
        // The portal's OpenURI is what `xdg-open` reaches in a sandbox; the url is on screen
        // already, so a computer with no opener loses nothing.
        let _ = std::process::Command::new("xdg-open")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// The sheets of one `accountd add`: the conversation on the terminal, and a consent that is the
/// person's own command.
#[derive(Debug)]
pub struct TerminalSheets<T> {
    inner: Arc<Inner<T>>,
}

#[derive(Debug)]
struct Inner<T> {
    terminal: T,
    /// The account the person's `--allow` is for, once it exists.
    allowing: Mutex<Option<AccountId>>,
}

impl<T> Clone for TerminalSheets<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: Terminal> TerminalSheets<T> {
    /// Sheets drawn on `terminal`.
    pub fn new(terminal: T) -> Self {
        Self {
            inner: Arc::new(Inner {
                terminal,
                allowing: Mutex::new(None),
            }),
        }
    }

    /// From now on a consent for `account` is answered `Allow`, always: the person typed
    /// `--allow` for it. A consent for any other account is dismissed.
    pub fn allow(&self, account: AccountId) {
        *self
            .inner
            .allowing
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(account);
    }

    /// The terminal.
    pub fn terminal(&self) -> &T {
        &self.inner.terminal
    }
}

impl<T: Terminal> Sheets for TerminalSheets<T> {
    type Link = TerminalLink<T>;

    async fn consent(&self, ask: ConsentAsk, _window: &ParentWindow) -> ConsentAnswer {
        let target = self
            .inner
            .allowing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match target.filter(|id| ask.accounts.iter().any(|a| a.account == *id)) {
            Some(account) => ConsentAnswer::Allow {
                account,
                scope: GrantScope::Always,
            },
            None => ConsentAnswer::Dismissed,
        }
    }

    async fn conversation(&self, open: SheetOpen) -> Result<TerminalLink<T>, SheetFault> {
        let mut link = TerminalLink {
            inner: Arc::clone(&self.inner),
            view: None,
        };
        link.update(open.view).await?;
        Ok(link)
    }
}

/// One conversation: what the sheet shows now decides what the next input asks.
#[derive(Debug)]
pub struct TerminalLink<T> {
    inner: Arc<Inner<T>>,
    view: Option<SheetView>,
}

/// What a field is called to the person.
fn label_of(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Address => "Email address",
        FieldKind::Server => "Server",
        FieldKind::Username => "User name",
        FieldKind::Password => "Password",
        FieldKind::ApiKey => "API key (hidden)",
        FieldKind::Token => "API token (hidden)",
    }
}

impl<T: Terminal> TerminalLink<T> {
    async fn ask(&self, label: String, echo: Echo) -> Result<String, SheetFault> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.terminal.ask(&label, echo))
            .await
            .ok()
            .flatten()
            .ok_or(SheetFault::Closed)
    }

    async fn fields(&self, view: porter_core::sheet::SignInView) -> Result<SheetInput, SheetFault> {
        let mut answers = Vec::new();
        for field in view.fields {
            let mut label = label_of(field.kind).to_owned();
            if field.presence == Presence::Optional {
                label.push_str(" (optional)");
            }
            let echo = match field.entry {
                Entry::Plain => Echo::On,
                Entry::Secret => Echo::Off,
            };
            let typed = self.ask(label, echo).await?;
            answers.push(FieldAnswer {
                kind: field.kind,
                value: match field.entry {
                    Entry::Plain => FieldValue::Plain(typed),
                    Entry::Secret => FieldValue::Secret(SecretText::new(typed)),
                },
            });
        }
        Ok(SheetInput::Submit(answers))
    }

    async fn review(&self, view: porter_core::sheet::ReviewView) -> Result<SheetInput, SheetFault> {
        let typed = self
            .ask("Add this account? [Y/n]".to_owned(), Echo::On)
            .await?;
        let no = matches!(typed.trim().to_ascii_lowercase().as_str(), "n" | "no");
        Ok(match no {
            true => SheetInput::Dismiss,
            false => SheetInput::Confirm(
                view.review
                    .services
                    .iter()
                    .filter(|row| matches!(row.state, ServiceState::Offered(_)))
                    .map(|row| ServiceChoice {
                        kind: row.kind,
                        toggle: Toggle::On,
                    })
                    .collect(),
            ),
        })
    }
}

impl<T: Terminal> SheetLink for TerminalLink<T> {
    async fn update(&mut self, view: SheetView) -> Result<(), SheetFault> {
        let terminal = &self.inner.terminal;
        match &view {
            SheetView::BrowserWait { url, .. } => {
                terminal.say("Continue in your browser. If it did not open, open this page:");
                terminal.say(url.as_str());
                terminal.open_browser(url.as_str());
            }
            SheetView::ShowCode { user_code, url, .. } => {
                terminal.say(&format!(
                    "Enter the code {} at {}",
                    user_code.0,
                    url.as_str()
                ));
            }
            SheetView::Review(review) => {
                terminal.say(&format!("Found {}:", review.review.label.0));
                for row in &review.review.services {
                    let state = match row.state {
                        ServiceState::Offered(_) => "available",
                        ServiceState::Absent(_) => "not offered",
                    };
                    terminal.say(&format!("  {:?}: {state}", row.kind));
                }
            }
            SheetView::SignIn(form) if form.problem.is_some() => {
                terminal.say("That was not accepted. Try again.");
            }
            SheetView::Failed { fault, .. } => {
                terminal.say(&format!("It did not work: {fault:?}."));
            }
            SheetView::Done => terminal.say("Added."),
            _ => {}
        }
        self.view = Some(view);
        Ok(())
    }

    async fn input(&mut self) -> Result<SheetInput, SheetFault> {
        // A view that waits on the browser or the provider has no input: it stays until the
        // sign-in moves on or the conversation is dropped.
        let waiting = matches!(
            self.view,
            Some(
                SheetView::BrowserWait { .. } | SheetView::ShowCode { .. } | SheetView::Working(_)
            )
        );
        if waiting {
            return std::future::pending().await;
        }
        match self.view.take() {
            Some(SheetView::SignIn(form)) => self.fields(form).await,
            Some(SheetView::Review(review)) => self.review(review).await,
            Some(SheetView::Failed { .. }) => Ok(SheetInput::Dismiss),
            _ => Err(SheetFault::Closed),
        }
    }
}

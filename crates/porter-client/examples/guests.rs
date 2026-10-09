//! Lists the computers that asked to use this computer's models and answers the first one that
//! is waiting: what Settings and the shell do. Run it as Settings or the shell (any other
//! caller is refused), with the session bus up.
//!
//! ```text
//! cargo run -p porter-client --example guests --features dbus -- allow
//! cargo run -p porter-client --example guests --features dbus -- deny
//! ```

use porter_client::lending::RowState;
use porter_client::{Accounts, ClientError, DbusTransport, GuestAnswer, GuestChange};

fn answer_of(word: Option<String>) -> Option<GuestAnswer> {
    GuestAnswer::from_slug(&word?)
}

async fn run(answer: Option<GuestAnswer>) -> Result<(), ClientError> {
    let accounts = Accounts::over(DbusTransport::session().await?);
    // Subscribe before reading the list, so no question falls between the two.
    let mut changes = accounts.watch_guests().await?;
    let rows = accounts.guests().await?;
    for row in &rows {
        println!("{} ({}): {}", row.name, row.node, row.state.slug());
    }
    if let (Some(answer), Some(waiting)) = (
        answer,
        rows.iter().find(|row| row.state == RowState::Asking),
    ) {
        accounts.answer_guest(&waiting.node, answer).await?;
        println!("{}: {}", waiting.name, answer.slug());
    }
    // Then listen: a computer that begins asking, or an answer that changes.
    while let Some(change) = changes.next().await {
        match change {
            GuestChange::Asks { node, ask } => {
                println!("{} ({node}) asks to use this computer's models", ask.name);
            }
            GuestChange::Changed => println!("the answers changed: read the list again"),
            _ => {}
        }
    }
    Ok(())
}

fn main() {
    let answer = answer_of(std::env::args().nth(1));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    if let Err(error) = runtime.block_on(run(answer)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

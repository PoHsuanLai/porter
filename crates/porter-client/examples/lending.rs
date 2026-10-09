//! What Settings does for the switch "Let my other computers use this computer's models": shows
//! whether the file that opens the network to inferd is installed and, when asked, installs or
//! takes it away. `enable` and `disable` change the real session's files and restart inferd, so
//! run them only on a computer where that is meant.
//!
//! ```text
//! cargo run -p porter-client --example lending --features lending,dbus
//! cargo run -p porter-client --example lending --features lending,dbus -- enable
//! cargo run -p porter-client --example lending --features lending,dbus -- disable
//! ```

use porter_client::{LendingConfig, LendingState, SessionUnits, TailnetLending};

async fn run(command: Option<String>) -> Result<(), String> {
    let config = LendingConfig::from_env().ok_or("there is no home folder")?;
    let units = SessionUnits::session().await.map_err(|e| e.to_string())?;
    let lending = TailnetLending::new(config, units);
    match command.as_deref() {
        Some("enable") => lending.enable().await.map_err(|e| e.to_string())?,
        Some("disable") => lending.disable().await.map_err(|e| e.to_string())?,
        _ => {}
    }
    let words = match lending.state() {
        LendingState::Installed => "on",
        LendingState::NotInstalled => "off",
        LendingState::Differs => "off (a different file is there; turning it on replaces it)",
        _ => "unknown",
    };
    println!("other computers may use this computer's models: {words}");
    Ok(())
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    if let Err(error) = runtime.block_on(run(std::env::args().nth(1))) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

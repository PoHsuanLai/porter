//! Serves a fake `org.freedesktop.secrets` on the session bus until Ctrl-C. Run it inside
//! `dbus-run-session` so it does not take the name from a real keyring.

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = zbus::Connection::session().await?;
    let _keyring = fake_secret_service::serve(&connection).await?;
    println!("serving {}", fake_secret_service::BUS_NAME);
    tokio::signal::ctrl_c().await?;
    Ok(())
}

//! Shared by the bus tests: a private bus and the real accountd on it.
#![allow(dead_code)]

#[cfg(feature = "dbus")]
pub mod accountd;
#[cfg(feature = "dbus")]
pub mod bus;
#[cfg(feature = "dbus")]
pub mod fake_accountd;

/// Waits until `condition` holds (a daemon's task has run); fails the test, naming `what`, when
/// `test_kit::GENEROUS` has passed by the clock.
pub async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = test_kit::Deadline::generous();
    while !deadline.passed() {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    deadline.fail(what);
}

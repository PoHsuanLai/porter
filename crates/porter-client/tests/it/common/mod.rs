//! Shared by the bus and socket tests: a private bus, a fake inferd and the real accountd on it,
//! the real session server over scripted seams, and a hand-written agent on a socket. What
//! speaks inference is behind the client's `infer` feature, as it is in the client.
#![allow(dead_code)]

#[cfg(feature = "dbus")]
pub mod accountd;
#[cfg(feature = "infer")]
pub mod agent;
#[cfg(feature = "dbus")]
pub mod bus;
#[cfg(all(feature = "dbus", feature = "infer"))]
pub mod inferd;
#[cfg(feature = "dbus")]
pub mod racing;
#[cfg(feature = "infer")]
pub mod served;

/// Waits until `condition` holds (a daemon's task has run); fails the test after five seconds.
pub async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

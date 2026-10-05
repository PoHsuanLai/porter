//! The sleep seam: a sign-in that polls (Login Flow v2 waits for the person in the browser)
//! waits through this, so no family reaches a runtime and a test waits for no time at all.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Waits for a duration.
pub trait Sleep: Send + Sync {
    /// Returns after `how_long`.
    fn sleep(&self, how_long: Duration) -> impl Future<Output = ()> + Send;
}

/// Does not wait. For tests, whose fakes answer at once.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSleep;

impl Sleep for NoSleep {
    async fn sleep(&self, _how_long: Duration) {}
}

trait DynSleep: Send + Sync {
    fn sleep_boxed(&self, how_long: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

impl<S: Sleep> DynSleep for S {
    fn sleep_boxed(&self, how_long: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.sleep(how_long))
    }
}

/// Any [`Sleep`], shared and cloneable, so a family holds one without a type parameter.
#[derive(Clone)]
pub struct SharedSleep(Arc<dyn DynSleep>);

impl SharedSleep {
    /// Shares `sleep`.
    pub fn new(sleep: impl Sleep + 'static) -> Self {
        Self(Arc::new(sleep))
    }
}

impl std::fmt::Debug for SharedSleep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedSleep")
    }
}

impl Sleep for SharedSleep {
    fn sleep(&self, how_long: Duration) -> impl Future<Output = ()> + Send {
        self.0.sleep_boxed(how_long)
    }
}

//! Closes a sheet whose wait was dropped, so an abandoned request leaves none on the screen.

use porter_dbus::Closer;

/// Armed with the sheet's [`Closer`] before the call that makes the sheet; disarmed (`.0 = None`)
/// once the answer is in. Dropped armed, it closes the sheet on the runtime if there is one.
pub(crate) struct Abandon(pub(crate) Option<Closer>);

impl Abandon {
    /// The answer is in: dropping it now closes nothing.
    pub(crate) fn answered(&mut self) {
        self.0 = None;
    }
}

impl Drop for Abandon {
    fn drop(&mut self) {
        let Some(closer) = self.0.take() else { return };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(closer.close());
        }
    }
}

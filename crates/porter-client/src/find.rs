//! `Accounts::find`, offered where the service is hosted: the in-process transport.

use crate::accounts::Accounts;
use crate::error::ClientError;
use crate::found::{ConsentOffer, Found, found};
use crate::{HostedCore, InProcess};
use porter_core::DataClass;
use porter_core::consent::Usage;
use porter_match::Need;

impl<C, B, H> Accounts<InProcess<C, B, H>>
where
    C: HostedCore,
{
    /// Whether an account can meet `need` for this app, and which.
    pub async fn find(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Result<Found, ClientError> {
        let candidates = self.transport.granted(need, class, usage);
        let offer = ConsentOffer {
            need: need.clone(),
            class,
            usage,
        };
        if !candidates.is_empty() {
            return Ok(found(
                candidates,
                porter_core::consent::Availability::Granted,
                offer,
            ));
        }
        let availability = self.transport.availability(need, class, usage);
        Ok(found(candidates, availability, offer))
    }
}

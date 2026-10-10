//! A hand-written `Manager` whose `Choose` answers before it returns, and lets a stranger answer
//! first: the two things a caller of a Request object must survive (a signal that beats the
//! method's reply, and one from a connection that does not own the name).

use porter_core::wire::Refusal;
use porter_core::{
    AccountId, AccountLabel, AccountsReply, Candidate, GrantId, ProviderId, Restriction, Subject,
};
use porter_dbus::{Details, NeedArg, REQUEST_INTERFACE, SheetKind, request_path, response_of};
use zbus::Connection;
use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;

/// What it does on `Choose`.
#[derive(Debug)]
pub struct Racing {
    /// A connection that is not the daemon, which sends a forged `Response` first.
    pub stranger: Option<Connection>,
}

pub fn candidate() -> Candidate {
    Candidate::new(
        AccountId::parse("fake-storage").expect("id"),
        AccountLabel("ada@cloud.invalid".into()),
        ProviderId::parse("fake-cloud").expect("provider"),
        Subject::Account,
        porter_fake::storage_account()
            .capabilities
            .iter()
            .find_map(|claim| match &claim.offer {
                porter_core::Offer::Present(capability) => Some(capability.clone()),
                _ => None,
            })
            .expect("a capability"),
        Restriction::none(),
        GrantId::parse("grant-1").expect("grant"),
    )
    .with_endpoints(porter_fake::storage_account().endpoints)
}

#[zbus::interface(name = "org.quire.Accounts1.Manager")]
impl Racing {
    #[allow(clippy::too_many_arguments)]
    async fn choose(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        _need: NeedArg,
        _class: String,
        _usage: String,
        _parent_window: String,
        options: Details,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let sender = header.sender().expect("a sender").to_owned();
        let token =
            String::try_from(options["handle_token"].try_clone().expect("clone")).expect("a token");
        let path = request_path(sender.as_str(), &token).expect("a request path");
        if let Some(stranger) = &self.stranger {
            let forged = response_of(
                SheetKind::Choose,
                &AccountsReply::Refused(Refusal::Dismissed),
            );
            stranger
                .emit_signal(
                    Some(sender.clone()),
                    path.as_str(),
                    REQUEST_INTERFACE,
                    "Response",
                    &(forged.code.to_wire(), forged.results),
                )
                .await?;
        }
        let genuine = response_of(SheetKind::Choose, &AccountsReply::Chosen(candidate()));
        // Before the reply: the signal is on the wire ahead of the method return.
        connection
            .emit_signal(
                Some(sender),
                path.as_str(),
                REQUEST_INTERFACE,
                "Response",
                &(genuine.code.to_wire(), genuine.results),
            )
            .await?;
        OwnedObjectPath::try_from(path).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
}

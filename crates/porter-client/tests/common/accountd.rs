//! accountd's three immediate interfaces on a bus, over the real `AccountService` core: the
//! thin adapter accountd itself will be. It answers for one fixed app (the bus-peer lookup is
//! accountd's, not the client's), maps each method onto an `AccountsRequest`, and a refusal onto
//! the error `org.quire.Accounts1.Error.<Refusal>`.

use porter_core::wire::Refusal;
use porter_core::{AccountsReply, AccountsRequest, AppId};
use porter_dbus::{
    CandidateArg, Details, NeedArg, TokenArg, candidate_to_dbus, grant_to_dbus, need_from_dbus,
    token_to_dbus,
};
use porter_fake::FakeService;
use std::sync::Arc;

/// One refusal as a D-Bus error. The names are the prefix plus the variant: they are
/// `refusal_error_name`'s, which a test pins.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.quire.Accounts1.Error")]
pub enum RefusedError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Dismissed(String),
    Denied(String),
    NoFittingAccount(String),
    UnknownGrant(String),
    AudienceNotGranted(String),
    NeedsReauth(String),
    Unavailable(String),
}

impl RefusedError {
    fn of(refusal: Refusal) -> Self {
        let text = String::new();
        match refusal {
            Refusal::Dismissed => Self::Dismissed(text),
            Refusal::Denied => Self::Denied(text),
            Refusal::NoFittingAccount => Self::NoFittingAccount(text),
            Refusal::UnknownGrant => Self::UnknownGrant(text),
            Refusal::AudienceNotGranted => Self::AudienceNotGranted(text),
            Refusal::NeedsReauth => Self::NeedsReauth(text),
            Refusal::Unavailable => Self::Unavailable(text),
        }
    }
}

fn bad(why: impl ToString) -> RefusedError {
    RefusedError::ZBus(zbus::Error::Failure(why.to_string()))
}

#[derive(Debug, Clone)]
pub struct Core {
    pub service: Arc<FakeService>,
    pub app: AppId,
}

impl Core {
    async fn handle(&self, request: AccountsRequest) -> Result<AccountsReply, RefusedError> {
        match self.service.handle(&self.app, request).await {
            AccountsReply::Refused(refusal) => Err(RefusedError::of(refusal)),
            other => Ok(other),
        }
    }

    /// Serves the three interfaces at the accountd path and takes the name.
    pub async fn serve(self, connection: &zbus::Connection) {
        let server = connection.object_server();
        server
            .at(porter_dbus::ACCOUNTS_PATH, Manager(self.clone()))
            .await
            .expect("manager");
        server
            .at(porter_dbus::ACCOUNTS_PATH, Grants(self.clone()))
            .await
            .expect("grants");
        server
            .at(porter_dbus::ACCOUNTS_PATH, Tokens(self))
            .await
            .expect("tokens");
        connection
            .request_name(porter_dbus::ACCOUNTS_BUS)
            .await
            .expect("name");
    }
}

fn slug<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, RefusedError> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).map_err(bad)
}

pub struct Manager(Core);
pub struct Grants(Core);
pub struct Tokens(Core);

#[zbus::interface(name = "org.quire.Accounts1.Manager")]
impl Manager {
    async fn query(
        &self,
        need: NeedArg,
        class: String,
        usage: String,
    ) -> Result<Vec<CandidateArg>, RefusedError> {
        let request = AccountsRequest::Query {
            need: need_from_dbus(need).map_err(bad)?,
            class: slug(&class)?,
            usage: slug(&usage)?,
        };
        match self.0.handle(request).await? {
            AccountsReply::Candidates(list) => Ok(list.iter().map(candidate_to_dbus).collect()),
            other => Err(bad(format!("{other:?}"))),
        }
    }

    async fn availability(
        &self,
        need: NeedArg,
        class: String,
        usage: String,
    ) -> Result<String, RefusedError> {
        let request = AccountsRequest::Availability {
            need: need_from_dbus(need).map_err(bad)?,
            class: slug(&class)?,
            usage: slug(&usage)?,
        };
        match self.0.handle(request).await? {
            AccountsReply::Availability(a) => serde_json::to_value(a)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(|| bad("slug")),
            other => Err(bad(format!("{other:?}"))),
        }
    }
}

#[zbus::interface(name = "org.quire.Accounts1.Grants")]
impl Grants {
    async fn list(&self) -> Result<Vec<(String, Details)>, RefusedError> {
        match self.0.handle(AccountsRequest::ListGrants).await? {
            AccountsReply::Grants(list) => Ok(list.iter().map(grant_to_dbus).collect()),
            other => Err(bad(format!("{other:?}"))),
        }
    }

    async fn revoke(&self, grant: String) -> Result<(), RefusedError> {
        let grant = porter_core::GrantId::parse(&grant).map_err(bad)?;
        match self.0.handle(AccountsRequest::Revoke { grant }).await? {
            AccountsReply::Revoked => Ok(()),
            other => Err(bad(format!("{other:?}"))),
        }
    }
}

#[zbus::interface(name = "org.quire.Accounts1.Tokens")]
impl Tokens {
    async fn issue_token(&self, grant: String, audience: String) -> Result<TokenArg, RefusedError> {
        let request = AccountsRequest::IssueToken {
            grant: porter_core::GrantId::parse(&grant).map_err(bad)?,
            audience: porter_core::Audience(audience),
        };
        match self.0.handle(request).await? {
            AccountsReply::Token(token) => Ok(token_to_dbus(&token)),
            other => Err(bad(format!("{other:?}"))),
        }
    }
}

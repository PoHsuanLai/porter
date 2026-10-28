//! Capabilities and claims the tests of this crate and of porter-match build from (feature
//! `testing`; always there in this crate's own tests).

use crate::offer::{Claim, Offer, Provenance, Subject};
use crate::{
    Access, Capability, Delta, HashKind, LabelModel, LlmCap, LlmFeature, LlmWire, MailCap,
    MailTransport, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_ids::Tokens;

/// A storage capability with the given access, change feed and reach.
pub fn storage(access: Access, delta: Delta, scope: StorageScope) -> Capability {
    Capability::Storage(StorageCap {
        access,
        delta,
        quota: QuotaReport::Reported,
        scope,
        hashes: HashKind::Sha1,
        ranges: Offered::Present,
        chunked_upload: Offered::Absent,
    })
}

/// A mail capability that offers `send` as given.
pub fn mail(send: Offered) -> Capability {
    Capability::Mail(MailCap {
        access: Access::ReadWrite,
        send,
        delta: Delta::Push,
        transport: MailTransport::Imap,
        labels: LabelModel::Folders,
    })
}

/// A language model with the given features and context window.
pub fn llm(features: &[LlmFeature], context: u32) -> Capability {
    Capability::Llm(LlmCap {
        features: features.iter().copied().collect(),
        context: Tokens(context),
        max_output: Tokens(4096),
        wire: LlmWire::ChatCompletions,
    })
}

/// A claim of `capability` made by the account itself, at `provenance`.
pub fn claim(capability: Capability, provenance: Provenance) -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Present(capability),
        provenance,
    }
}

//! Capabilities the unit tests build from (test-only).

use crate::capability::{
    Access, Capability, Delta, HashKind, LabelModel, LlmCap, LlmFeature, LlmWire, MailCap,
    MailTransport, Offered, QuotaReport, StorageCap, StorageScope,
};
use crate::offer::{Claim, Offer, Provenance, Subject};
use crate::units::Tokens;

pub(crate) fn storage(access: Access, delta: Delta, scope: StorageScope) -> Capability {
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

pub(crate) fn mail(send: Offered) -> Capability {
    Capability::Mail(MailCap {
        access: Access::ReadWrite,
        send,
        delta: Delta::Push,
        transport: MailTransport::Imap,
        labels: LabelModel::Folders,
    })
}

pub(crate) fn llm(features: &[LlmFeature], context: u32) -> Capability {
    Capability::Llm(LlmCap {
        features: features.iter().copied().collect(),
        context: Tokens(context),
        max_output: Tokens(4096),
        wire: LlmWire::ChatCompletions,
    })
}

pub(crate) fn claim(capability: Capability, provenance: Provenance) -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Present(capability),
        provenance,
    }
}

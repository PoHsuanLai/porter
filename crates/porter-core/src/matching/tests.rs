use super::*;
use crate::capability::{
    Access, Albums, CapabilityKind, Delta, LibraryRead, LlmFeature, Offered, PhotosCap,
    QuotaReport, StorageScope,
};
use crate::fixtures::{llm, mail, storage};
use crate::need::{LlmNeed, MailNeed, PhotosNeed, StorageNeed};
use crate::units::Tokens;

fn storage_need(access: Access, delta: Delta, scope: StorageScope) -> Need {
    Need::Storage(StorageNeed {
        access,
        delta,
        scope,
        quota: QuotaReport::Unreported,
    })
}

fn present(capability: Capability) -> Offer {
    Offer::Present(capability)
}

fn google_photos() -> Offer {
    present(Capability::Photos(PhotosCap {
        library_read: LibraryRead::PickerOnly,
        upload: Offered::Present,
        albums: Albums::AppCreated,
        video: Offered::Present,
        delta: Delta::None,
    }))
}

fn cases() -> Vec<(&'static str, Need, Offer, Match)> {
    use Access::{Read, ReadWrite};
    use StorageScope::{AppFolder, Full};
    let chat_tools: &[LlmFeature] = &[LlmFeature::Chat, LlmFeature::Tools];
    vec![
        (
            "storage write+poll fits a push store",
            storage_need(ReadWrite, Delta::Poll, AppFolder),
            present(storage(ReadWrite, Delta::Push, Full)),
            Match::Fits,
        ),
        (
            "equal minimums fit",
            storage_need(ReadWrite, Delta::Poll, AppFolder),
            present(storage(ReadWrite, Delta::Poll, AppFolder)),
            Match::Fits,
        ),
        (
            "read-only store falls short on access",
            storage_need(ReadWrite, Delta::Poll, AppFolder),
            present(storage(Read, Delta::Push, Full)),
            Match::Short(Shortfall::Access),
        ),
        (
            "no-delta store falls short on delta",
            storage_need(Read, Delta::Poll, AppFolder),
            present(storage(ReadWrite, Delta::None, Full)),
            Match::Short(Shortfall::Delta),
        ),
        (
            "app folder falls short of a full-store need",
            storage_need(Read, Delta::None, Full),
            present(storage(ReadWrite, Delta::Push, AppFolder)),
            Match::Short(Shortfall::Scope),
        ),
        (
            "first shortfall wins",
            storage_need(ReadWrite, Delta::Push, Full),
            present(storage(Read, Delta::None, AppFolder)),
            Match::Short(Shortfall::Access),
        ),
        (
            "absent storage says why",
            storage_need(Read, Delta::None, AppFolder),
            Offer::Absent {
                kind: CapabilityKind::Storage,
                reason: AbsentReason::ProviderOffersNone,
            },
            Match::Absent(AbsentReason::ProviderOffersNone),
        ),
        (
            "absent of another kind is another kind",
            storage_need(Read, Delta::None, AppFolder),
            Offer::Absent {
                kind: CapabilityKind::Notes,
                reason: AbsentReason::TenantConsent,
            },
            Match::OtherKind,
        ),
        (
            "mail offer does not answer a storage need",
            storage_need(Read, Delta::None, AppFolder),
            present(mail(Offered::Present)),
            Match::OtherKind,
        ),
        (
            "mail without send falls short on send",
            Need::Mail(MailNeed {
                access: Read,
                send: Offered::Present,
                delta: Delta::None,
            }),
            present(mail(Offered::Absent)),
            Match::Short(Shortfall::Send),
        ),
        (
            "google photos cannot mirror a library",
            Need::Photos(PhotosNeed {
                library_read: LibraryRead::Full,
                upload: Offered::Absent,
                albums: Albums::None,
                video: Offered::Absent,
                delta: Delta::None,
            }),
            google_photos(),
            Match::Short(Shortfall::LibraryRead),
        ),
        (
            "google photos is an upload target",
            Need::Photos(PhotosNeed {
                library_read: LibraryRead::None,
                upload: Offered::Present,
                albums: Albums::AppCreated,
                video: Offered::Absent,
                delta: Delta::None,
            }),
            google_photos(),
            Match::Fits,
        ),
        (
            "llm with the features and window fits",
            Need::Llm(LlmNeed {
                features: chat_tools.iter().copied().collect(),
                context: Tokens(32_000),
            }),
            present(llm(
                &[LlmFeature::Chat, LlmFeature::Tools, LlmFeature::Vision],
                128_000,
            )),
            Match::Fits,
        ),
        (
            "llm without tools falls short on features",
            Need::Llm(LlmNeed {
                features: chat_tools.iter().copied().collect(),
                context: Tokens(8_000),
            }),
            present(llm(&[LlmFeature::Chat], 128_000)),
            Match::Short(Shortfall::Features),
        ),
        (
            "llm with a small window falls short on context",
            Need::Llm(LlmNeed {
                features: Default::default(),
                context: Tokens(32_000),
            }),
            present(llm(&[LlmFeature::Chat], 8_000)),
            Match::Short(Shortfall::Context),
        ),
    ]
}

#[test]
fn matches_answers_each_need_against_each_offer() {
    for (name, need, offer, expected) in cases() {
        assert_eq!(matches(&need, &offer), expected, "{name}");
    }
}

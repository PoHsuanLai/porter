use super::*;
use porter_core::capability::{Access, Delta, Offered, PimCap};
use porter_core::{
    AccountId, AccountLabel, EndpointUrl, Family, GrantId, LoginName, ProviderId, Restriction,
    Subject, Tls,
};

fn cap(transport: PimTransport) -> PimCap {
    PimCap {
        access: Access::Read,
        delta: Delta::Poll,
        transport,
        collections: Offered::Present,
    }
}

fn candidate(provider: &str, capability: Capability) -> Candidate {
    Candidate {
        account: AccountId::parse("a1").expect("account"),
        label: AccountLabel("a@b.test".into()),
        provider: ProviderId::parse(provider).expect("provider"),
        subject: Subject::Account,
        capability,
        restriction: Restriction::none(),
        grant: GrantId::parse("g1").expect("grant"),
        endpoints: vec![ServiceEndpoint {
            family: Family::Graph,
            url: EndpointUrl::parse("https://graph.example.test").expect("url"),
            tls: Tls::Plain,
            login: LoginName("a".into()),
        }],
    }
}

#[test]
fn the_source_is_chosen_by_the_capabilitys_transport_never_by_the_provider() {
    use PimTransport as P;
    let calendar = |t| Capability::Calendar(cap(t));
    let contacts = |t| Capability::Contacts(cap(t));
    // The same provider name with each transport; a provider called "microsoft" or "google" is
    // not special.
    for provider in ["microsoft", "google", "anything"] {
        let cases = [
            (PimKind::Calendar, calendar(P::CalDav), Ok(Chosen::Dav)),
            (PimKind::Contacts, contacts(P::CardDav), Ok(Chosen::Dav)),
            (
                PimKind::Calendar,
                calendar(P::Graph),
                Ok(Chosen::GraphCalendar),
            ),
            (
                PimKind::Contacts,
                contacts(P::Graph),
                Err(NoSource {
                    kind: PimKind::Contacts,
                    transport: P::Graph,
                }),
            ),
            (
                PimKind::Calendar,
                calendar(P::GoogleApi),
                Ok(Chosen::GoogleCalendar),
            ),
            (
                PimKind::Contacts,
                contacts(P::GoogleApi),
                Ok(Chosen::GooglePeople),
            ),
            (
                PimKind::Tasks,
                Capability::Tasks(cap(P::GoogleApi)),
                Ok(Chosen::GoogleTasks),
            ),
            // A task list on a CalDAV account is a calendar of VTODOs, mirrored as one.
            (
                PimKind::Tasks,
                Capability::Tasks(cap(P::CalDav)),
                Err(NoSource {
                    kind: PimKind::Tasks,
                    transport: P::CalDav,
                }),
            ),
            (
                PimKind::Tasks,
                Capability::Tasks(cap(P::Graph)),
                Err(NoSource {
                    kind: PimKind::Tasks,
                    transport: P::Graph,
                }),
            ),
            (
                PimKind::Calendar,
                calendar(P::Jmap),
                Err(NoSource {
                    kind: PimKind::Calendar,
                    transport: P::Jmap,
                }),
            ),
        ];
        for (kind, capability, want) in cases {
            let got = choose(kind, &candidate(provider, capability.clone()));
            assert_eq!(got, want, "{provider} {kind:?} {capability:?}");
        }
    }
}

#[test]
fn a_cursor_text_that_is_not_the_sources_is_not_a_cursor() {
    assert_eq!(
        graph::DeltaLink::decode("https://g.test/x?t=1")
            .map(|c| c.encode())
            .as_deref(),
        Some("https://g.test/x?t=1")
    );
    assert!(graph::DeltaLink::decode("sync-token-1").is_none());
    assert_eq!(
        dav::DavCursor::decode("sync-token-1")
            .map(|c| c.encode())
            .as_deref(),
        Some("sync-token-1")
    );
}

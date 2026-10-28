use super::*;
use crate::fixtures;

/// Every kind with the slug it has in files, on the bus and in the consent store, the name a
/// person reads and whether the broker routes it, in the order of the list.
const KINDS: &[(CapabilityKind, &str, &str, bool)] = &[
    (
        CapabilityKind::Identity,
        "identity",
        "Account details",
        false,
    ),
    (CapabilityKind::Mail, "mail", "Mail", false),
    (CapabilityKind::Calendar, "calendar", "Calendar", false),
    (CapabilityKind::Contacts, "contacts", "Contacts", false),
    (CapabilityKind::Tasks, "tasks", "Tasks", false),
    (CapabilityKind::Notes, "notes", "Notes", false),
    (CapabilityKind::Storage, "storage", "Files", false),
    (CapabilityKind::Photos, "photos", "Photos", false),
    (CapabilityKind::Llm, "llm", "Language model", true),
    (
        CapabilityKind::Embeddings,
        "embeddings",
        "Search by meaning",
        true,
    ),
    (CapabilityKind::Speech, "speech", "Speech", true),
    (
        CapabilityKind::ImageGen,
        "image_gen",
        "Image generation",
        true,
    ),
    (CapabilityKind::Rerank, "rerank", "Result ranking", true),
    (
        CapabilityKind::ComputerUse,
        "computer_use",
        "Operating windows",
        true,
    ),
    (
        CapabilityKind::KeyValue,
        "key_value",
        "Small synced items",
        false,
    ),
    (CapabilityKind::Push, "push", "Notifications", false),
    (CapabilityKind::Agent, "agent", "Assistant", false),
];

#[test]
fn the_list_is_every_kind_in_order_with_the_slug_name_and_routing_it_always_had() {
    let listed: Vec<CapabilityKind> = KINDS.iter().map(|(kind, ..)| *kind).collect();
    assert_eq!(CapabilityKind::ALL, listed.as_slice());
    let mut sorted = listed.clone();
    sorted.sort();
    assert_eq!(
        sorted, listed,
        "the order of the list is the order of the kinds"
    );
    for (kind, slug, name, ai) in KINDS {
        assert_eq!(
            serde_json::to_string(kind).expect("json"),
            format!("\"{slug}\"")
        );
        assert_eq!(
            serde_json::from_str::<CapabilityKind>(&format!("\"{slug}\"")).expect("kind"),
            *kind
        );
        assert_eq!(kind.name(), *name, "{slug}");
        assert_eq!(kind.is_ai(), *ai, "{slug}");
    }
}

#[test]
fn each_service_type_is_its_kind() {
    assert_eq!(service::Mail::KIND, CapabilityKind::Mail);
    assert_eq!(service::Calendar::KIND, CapabilityKind::Calendar);
    assert_eq!(service::Contacts::KIND, CapabilityKind::Contacts);
    assert_eq!(service::Tasks::KIND, CapabilityKind::Tasks);
    assert_eq!(service::Agent::KIND, CapabilityKind::Agent);
    assert_eq!(service::Llm::NAME, "Language model");
    const { assert!(service::ComputerUse::AI) };
    const { assert!(!service::Storage::AI) };
}

#[test]
fn a_capability_is_written_as_its_kind_and_its_fields() {
    let capability = fixtures::mail(true);
    let json = serde_json::to_value(&capability).expect("json");
    assert_eq!(json["kind"], "mail");
    assert!(json["v"].is_object());
    assert_eq!(capability.kind(), CapabilityKind::Mail);
    let back: Capability = serde_json::from_value(json).expect("capability");
    assert_eq!(back, capability);
}

//! The ids of the accounts vocabulary, each in the one slug grammar of `porter-ids`.

use porter_ids::slug_id;

slug_id!(
    /// One configured account (a row in accountd's registry, or a discovered local runtime).
    AccountId,
    "account id"
);
slug_id!(
    /// One provider declaration (`providers/<id>.toml`).
    ProviderId,
    "provider id"
);
slug_id!(
    /// One consent decision, named so an app can revoke it and Settings can list it.
    GrantId,
    "grant id"
);
slug_id!(
    /// One person's yes for an app to use one of their AI keys: what the key host (inferd) keeps
    /// in its registry as a grant of kind Llm. The same text as the [`GrantId`] it is stored under.
    KeyPermitId,
    "key permit id"
);

impl From<GrantId> for KeyPermitId {
    fn from(grant: GrantId) -> Self {
        Self(grant.into())
    }
}

impl From<KeyPermitId> for GrantId {
    fn from(permit: KeyPermitId) -> Self {
        // Both are the one id grammar, so the text is always a well-formed grant id.
        GrantId::parse(permit.as_str()).expect("a permit id is a grant id")
    }
}

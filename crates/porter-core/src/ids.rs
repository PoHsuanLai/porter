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
    /// One consent decision, named so an app can revoke it and Settings can list it. A person's yes
    /// for an app to use one of their AI keys (a permit) is the same grant, stored with kind Llm.
    GrantId,
    "grant id"
);

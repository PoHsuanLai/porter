//! The id of a model an AI account serves.

use porter_ids::slug_id;

slug_id!(
    /// One model an AI account serves (`llama3.2`, `claude-sonnet-4-5`).
    ModelId,
    "model id"
);

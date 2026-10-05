//! Helpers for the machine tests: feed bytes, read back what was sent.

use crate::step::{Effect, Input, Relaying, Side};

/// Feeds `text` one byte at a time, as the worst read splitting would, and gathers the effects.
pub(crate) fn bytewise<M: Relaying>(mut machine: M, from: Side, text: &[u8]) -> (M, Vec<Effect>) {
    let mut all = Vec::new();
    for byte in text {
        let (next, effects) = machine.step(Input::Bytes {
            from,
            data: vec![*byte],
        });
        machine = next;
        all.extend(effects);
    }
    (machine, all)
}

/// Feeds `text` whole.
pub(crate) fn whole<M: Relaying>(machine: M, from: Side, text: &[u8]) -> (M, Vec<Effect>) {
    machine.step(Input::Bytes {
        from,
        data: text.to_vec(),
    })
}

/// What was sent to `side`, in order, as text.
pub(crate) fn sent(effects: &[Effect], side: Side) -> String {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Send { to, data } if *to == side => Some(String::from_utf8_lossy(data)),
            _ => None,
        })
        .collect()
}

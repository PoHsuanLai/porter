//! The relay: one machine driven over two streams and a connector.

use crate::connect::Connect;
use crate::step::RelayEnd;
use porter_core::RelayPlan;
use porter_core::stream::ByteStream;

/// Dials the plan's endpoint through `connect`, runs the machine of its protocol and relays
/// until either side finishes. The relay holds the plan, and so the credential, for as long as
/// it runs and never writes it anywhere but to the endpoint.
pub async fn relay<A: ByteStream, C: Connect>(plan: RelayPlan, app: A, connect: &C) -> RelayEnd {
    let _ = (plan, app, connect);
    todo!(
        "pick `ImapRelay`, `SmtpRelay` or `HttpRelay` by `plan.endpoint.protocol()`, dial, feed \
         `Input::Start`, then loop: read from either side, `step`, carry out each effect \
         (`Send`, `StartTls` through `Connect::upgrade`, `Close`)"
    )
}

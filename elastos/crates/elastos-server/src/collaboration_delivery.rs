//! The one delivery engine under every Runtime-owned collaboration send.
//!
//! Four shipped behaviours deliver durable artifacts to a peer's device and
//! settle them against verified acknowledgements: direct chat messages,
//! signed contact revocations, signed Profile update announcements, and the
//! shared-room gossip outbox. This module is the layer they share — the
//! bounded retry pass and the declared end-of-life contract — so a new
//! transport (the encrypted mailbox next) extends one engine instead of
//! adding a fifth hand-rolled loop.
//!
//! What deliberately stays different, and why:
//!
//! - **Artifact storage.** Each artifact lives where its authority lives:
//!   the shared-room outbox in the conversation core, direct messages in the
//!   principal's message store, a revocation on the relationship record it
//!   ends, and a Profile announcement nowhere at all — it is a pure function
//!   of the durable Profile head and the accepted contacts. Collapsing those
//!   stores would cross the bounded-store authority boundary for no product
//!   gain; the engine runs over them instead.
//! - **Selection.** Which artifacts are eligible is a product rule per
//!   source (live pair endpoints only, unexpired envelopes only, contacts
//!   that have not acknowledged the head), so each source builds its own
//!   bounded plan and hands it to the shared pass.
//! - **The shared-room outbox** is broadcast-with-asynchronous-receipts over
//!   gossip, not a request/response exchange, so it uses the shared envelope
//!   and receipt primitives and declares its end-of-life here, but does not
//!   run the request/response pass.

use std::future::Future;

/// What a delivery source does when an artifact's envelope reaches the end
/// of its signed lifetime. This is a product decision each source declares
/// once, never an emergent behaviour of its retry loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryEndOfLife {
    /// The artifact becomes terminal and visibly `expired`; the Runtime
    /// never re-delivers it. Direct chat messages declare this: a message
    /// the peer never acknowledged within its lifetime is abandoned, and the
    /// read model says so instead of pretending it is still pending.
    TerminalExpired,
    /// The Runtime mints a fresh envelope carrying the exact same signed
    /// fact and keeps delivering. Contact revocations declare this: a
    /// removal must eventually reach the removed peer, however long they
    /// stay offline.
    RemintExact,
    /// Nothing durable expires because nothing durable exists: every pass
    /// regenerates the envelope from current signed truth. Profile update
    /// announcements declare this — and the shared-room outbox shares the
    /// terminal side of the direct contract for its own expired envelopes.
    RegenerateFromTruth,
}

/// One artifact a source selected for this pass.
pub(crate) struct DeliveryPlanItem {
    /// Source-scoped identity for settlement — a conversation, a removed
    /// pair, a contact's Profile DID. Never routing material.
    pub(crate) key: String,
    pub(crate) envelope: Vec<u8>,
    pub(crate) recipient_endpoint_did: String,
}

/// What one delivery attempt produced.
pub(crate) enum DeliveryAttempt {
    /// The selected peer endpoint returned a verified acknowledgement; the pass calls
    /// the source's settle hook.
    Settled,
    /// Transport could not reach the peer right now. Not an error by itself:
    /// the artifact stays pending and the cadence retries.
    Unreachable,
}

/// Runs one bounded delivery pass: attempt every planned item, settle the
/// acknowledged ones, and report the first failure after finishing the whole
/// plan — one unreachable peer never starves the rest of the plan, and one
/// success never masks a failure. Every request/response collaboration
/// retry loop is this function; sources differ only in how they select the
/// plan and what settling means.
pub(crate) async fn run_bounded_delivery_pass<D, DF, S>(
    plan: Vec<DeliveryPlanItem>,
    mut deliver: D,
    mut settle: S,
) -> anyhow::Result<()>
where
    D: FnMut(DeliveryPlanItem) -> DF,
    DF: Future<Output = (DeliveryPlanItem, anyhow::Result<DeliveryAttempt>)>,
    S: FnMut(&DeliveryPlanItem) -> anyhow::Result<()>,
{
    let mut first_failure: Option<anyhow::Error> = None;
    for item in plan {
        let (item, outcome) = deliver(item).await;
        match outcome {
            Ok(DeliveryAttempt::Settled) => {
                if let Err(err) = settle(&item) {
                    first_failure.get_or_insert(err);
                }
            }
            Ok(DeliveryAttempt::Unreachable) => {
                first_failure.get_or_insert_with(|| {
                    anyhow::anyhow!("collaboration delivery transport is unavailable")
                });
            }
            Err(err) => {
                first_failure.get_or_insert(err);
            }
        }
    }
    first_failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn item(key: &str) -> DeliveryPlanItem {
        DeliveryPlanItem {
            key: key.to_string(),
            envelope: b"envelope".to_vec(),
            recipient_endpoint_did: "did:key:zPeer".to_string(),
        }
    }

    #[tokio::test]
    async fn pass_settles_only_acknowledged_items_and_reports_first_failure_last() {
        let settled = Arc::new(std::sync::Mutex::new(Vec::new()));
        let attempts = Arc::new(AtomicUsize::new(0));
        let settled_hook = settled.clone();
        let attempts_hook = attempts.clone();
        let result = run_bounded_delivery_pass(
            vec![item("a"), item("b"), item("c"), item("d")],
            move |plan_item| {
                let attempts = attempts_hook.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    let outcome = match plan_item.key.as_str() {
                        "a" => Ok(DeliveryAttempt::Settled),
                        "b" => Err(anyhow::anyhow!("peer rejected b")),
                        "c" => Ok(DeliveryAttempt::Unreachable),
                        _ => Ok(DeliveryAttempt::Settled),
                    };
                    (plan_item, outcome)
                }
            },
            |plan_item| {
                settled_hook.lock().unwrap().push(plan_item.key.clone());
                Ok(())
            },
        )
        .await;
        // The whole plan ran despite the failure in the middle...
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        // ...only acknowledged items settled...
        assert_eq!(*settled.lock().unwrap(), vec!["a", "d"]);
        // ...and the first failure is what the caller sees.
        assert!(result.unwrap_err().to_string().contains("peer rejected b"));
    }

    #[tokio::test]
    async fn unreachable_alone_is_reported_without_masking_settlements() {
        let result = run_bounded_delivery_pass(
            vec![item("a")],
            |plan_item| async move { (plan_item, Ok(DeliveryAttempt::Unreachable)) },
            |_| Ok(()),
        )
        .await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("transport is unavailable"));
    }

    #[tokio::test]
    async fn empty_plan_is_a_clean_pass_and_settle_failures_are_failures() {
        run_bounded_delivery_pass(
            Vec::new(),
            |item| async move { (item, unreachable_ok()) },
            |_| Ok(()),
        )
        .await
        .unwrap();
        let result = run_bounded_delivery_pass(
            vec![item("a")],
            |plan_item| async move { (plan_item, Ok(DeliveryAttempt::Settled)) },
            |_| Err(anyhow::anyhow!("settlement store is read-only")),
        )
        .await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("settlement store is read-only"));
    }

    fn unreachable_ok() -> anyhow::Result<DeliveryAttempt> {
        Ok(DeliveryAttempt::Unreachable)
    }
}

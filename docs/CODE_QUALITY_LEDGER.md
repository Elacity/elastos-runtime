# Code-quality ledger — deferred structural cleanups (Flint)

The 2026-07 four-seat quality audit (money spine, dispatch surface, receipt primitives,
docs/presentability) folded every small-and-safe finding directly (shared sentinel consts, the
ledger admission helper, whole-record rollback, frozen-serialization docs on `AuditEvent`, the
shared canonicalization recipe, env-lock ordering, doc restructuring — see the audit commit).
The findings below are REAL but structural — multi-day, behavior-preserving refactors that
deserve their own gated increments rather than a rushed fold. Honest status: none of these is a
correctness bug; all fail in the safe direction today.

| # | Area | Finding | Shape of the fix |
|---|---|---|---|
| CQ-1 | `api/handlers/capability.rs` (6.3k lines) | Four separable products in one file: consent-request flow, mandate lifecycle + dispatch, money provisioning/reconciliation, and a 3.8k-line test module | Split into `capability.rs` / `mandates.rs` / `money.rs` with re-exports through `handlers`; pure code motion |
| CQ-2 | `dispatch_standing_intent` (~335 lines) | Eight pipeline stages whose ordering invariants live in prose comments; three `Arc` side channels smuggle executor results | Named per-stage helpers returning `Result<(), (StatusCode, String)>` + one `ExecSideChannel` struct; ~40-line orchestrator |
| CQ-3 | Test fixtures (capability.rs, intent_executor.rs, manager.rs, gateway_mandates.rs) | The 8-field `IssueStandingGrantInput` literal ×~25, five near-identical state constructions, six `IntentDeclarationV1::issue` scaffolds, ~30 copy-pasted `validate()` calls | `#[cfg(test)]` builders (`grant_input`, `signed_intent`, `state_with_rail`) collapsing each test to its intent |
| CQ-4 | Error types at seams | `(StatusCode, String)` across ~40 handlers; `AuditError::Serialize(String)/Io(String)`; `verify_chain -> Result<u64, String>`; three coexisting handler-error styles | One crate-internal `ApiError { status, code, message }: IntoResponse`; structured `ChainVerifyError` variants with identical `Display` texts |
| CQ-5 | `capability/intent.rs` (3.6k lines) | Seven jobs in one module (records, envelope, gate, store + migrations, dispatcher, service, tests) | Split into `intent/{records,envelope,gate,store,service}.rs` with a re-exporting `mod.rs`; pure code motion |
| CQ-6 | flock + atomic-snapshot persistence | The single-opener flock block and tmp+fsync+rename discipline exist in `SpendMeter`, `PaymentLedger`, and `StandingGrantStore`, already drifted (the ledger lacks the pre/post-publish split) | Shared `acquire_single_opener` / `write_snapshot_atomic` helpers in a common crate; each store keeps its own rollback/poison policy |
| CQ-7 | `primitives/audit.rs` read surface | Six hand-rolled open-log→parse-`ChainedRecord` loops with divergent error handling; receipt exporters swallow I/O errors into `None` (`.ok()?`) | One `chained_records(path)` iterator; exporters return `Result<Option<MandateReceipt>, AuditError>` |
| CQ-8 | `event_type_name` (30 arms) | Hand-maintained mirror of serde's tag strings; a drifted arm silently breaks event filtering | Exhaustive test asserting `event_type_name() == serde_json::to_value(ev)["type"]` for every variant (or generate via serde) |
| CQ-9 | Sprint/council archaeology in comments | Ticket-style IDs ("council S29 G-F8", "chunk 2c-gw-A") bury real invariants for outside readers | Keep the constraint sentence, cite the decision log once per module; targeted sweep, never touching serialized fields |
| CQ-10 | Misc | `content_open`'s 7 positional params (+stringly `"opened"/"denied"`); `BuyOutcome.unsigned_tx` misnamed in wallet-signed modes; `with_file_handle` test seam is plain `pub`; double serialization per audit emit; `mandate_cmd.rs` display shaping untested; chain-tx helpers re-resolve the binary per call | Params struct + `Decision` enum; rename/re-doc `tx_view`; `#[doc(hidden)]`; `RawValue` single-serialize; extract + unit-test display shaping; thread a `LiveChain` struct |

Rule for closing an entry: the refactor ships in its own commit with the full gate green and, where
the entry names a drift risk, a ratchet test that pins the invariant (CQ-8's exhaustive test is the
model). Delete the row when it lands.

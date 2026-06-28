/**
 * ESP v0 — the hero consent act (the dDRM open), as projection/journey shapes.
 *
 * The hero act flows perceive → plan → consent → act → audit through W2:
 *   invoke (consent-gated) → 202 `AffordanceConsentPending` → approve →
 *   retry with the granted token → `validate-and-consume` → signed receipt.
 *
 * These helpers are what a shell uses to (a) recognise a well-formed consent
 * request and (b) prove the receipt it got back attests the SAME act it asked
 * for. The shell holds no key and does no crypto — the runtime signed and
 * verified; the shell asserts the receipt is FOR this act and renders it.
 */

import { ESP_SCHEMA_TAGS } from "./esp_v0.js";
import type {
  AffordanceConsentPending,
  ValidateAndConsumeInput,
  ValidateAndConsumeOutput,
} from "./esp_v0.js";

/** A consent-pending fact is well-formed for the hero act (W2). */
export function consentPendingIsWellFormed(pending: AffordanceConsentPending): boolean {
  return (
    pending.schema === ESP_SCHEMA_TAGS.affordanceConsentPending &&
    pending.status === "approval_pending" &&
    pending.request_id.length > 0 &&
    pending.capsule.length > 0 &&
    pending.method.length > 0
  );
}

/**
 * The hero act's safety check: the redemption receipt must attest the EXACT act
 * the caller asked to perform — same method, resource, and action — and report
 * the single use as consumed. A mismatch means the shell must NOT render the act
 * as done. (The cryptographic verification of the signature is the runtime's job;
 * this is the shell asserting the receipt is bound to THIS request.)
 */
export function receiptMatchesRequest(
  request: ValidateAndConsumeInput,
  result: ValidateAndConsumeOutput,
): boolean {
  return (
    result.status === "consumed" &&
    result.receipt.schema === ESP_SCHEMA_TAGS.affordanceReceipt &&
    result.receipt.method_id === request.method_id &&
    result.receipt.resource === request.resource &&
    result.receipt.action === request.action
  );
}

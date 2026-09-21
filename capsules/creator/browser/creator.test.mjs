import assert from "node:assert/strict";
import test from "node:test";

import {
  buildPublishBody,
  discardProtectionAndRetry,
  classifyProtection,
  decimalIntegerToHexQuantity,
  humanSize,
  effectPendingFrom,
  resolveMime,
  scalePriceToBaseUnits,
  creatorMintFrom,
  pendingBudgetMs,
  pendingPhase,
  failureDetailRows,
  stagesFromProgress,
  settledFrom,
  summarizeRoyaltyRows,
  targetUriFor,
  uploadPlan,
} from "./creator.js";

test("targetUriFor builds a Creator-folder object URI", () => {
  assert.equal(
    targetUriFor("localhost://principal/did-key-abc", "clip.mp4"),
    "localhost://principal/did-key-abc/Creator/clip.mp4",
  );
});

test("targetUriFor strips a trailing slash from the root URI", () => {
  assert.equal(
    targetUriFor("localhost://principal/did-key-abc/", "clip.mp4"),
    "localhost://principal/did-key-abc/Creator/clip.mp4",
  );
});

test("targetUriFor rejects an empty file name", () => {
  assert.throws(() => targetUriFor("localhost://root", ""));
});

test("targetUriFor rejects a bare dot-dot file name", () => {
  assert.throws(() => targetUriFor("localhost://root", ".."));
});

test("targetUriFor rejects a file name containing a path traversal segment", () => {
  assert.throws(() => targetUriFor("localhost://root", "../secret.txt"));
});

test("targetUriFor rejects a file name containing a path separator", () => {
  assert.throws(() => targetUriFor("localhost://root", "a/b.txt"));
});

test("targetUriFor rejects a file name containing a control character", () => {
  const newline = String.fromCharCode(10);
  const nul = String.fromCharCode(0);
  const del = String.fromCharCode(127);
  assert.throws(() => targetUriFor("localhost://root", `clip${newline}name.mp4`));
  assert.throws(() => targetUriFor("localhost://root", `clip${nul}name.mp4`));
  assert.throws(() => targetUriFor("localhost://root", `clip${del}name.mp4`));
});

test("classifyProtection treats video as media", () => {
  assert.equal(classifyProtection("video/mp4"), "media");
});

test("classifyProtection treats audio as media", () => {
  assert.equal(classifyProtection("audio/mpeg"), "media");
});

test("classifyProtection treats everything else as object", () => {
  assert.equal(classifyProtection("application/pdf"), "object");
  assert.equal(classifyProtection(""), "object");
  assert.equal(classifyProtection(undefined), "object");
});

test("decimalIntegerToHexQuantity converts a positive decimal integer to hex", () => {
  assert.equal(decimalIntegerToHexQuantity("1"), "0x1");
  assert.equal(decimalIntegerToHexQuantity("255"), "0xff");
});

test("decimalIntegerToHexQuantity rejects zero", () => {
  assert.throws(() => decimalIntegerToHexQuantity("0"));
});

test("decimalIntegerToHexQuantity rejects a non-numeric value", () => {
  assert.throws(() => decimalIntegerToHexQuantity("abc"));
  assert.throws(() => decimalIntegerToHexQuantity("1.5"));
  assert.throws(() => decimalIntegerToHexQuantity("-1"));
});

test("decimalIntegerToHexQuantity rejects a value above uint256 max", () => {
  const tooLarge = (1n << 256n).toString(10);
  assert.throws(() => decimalIntegerToHexQuantity(tooLarge));
});

test("uploadPlan chooses a single request at or under the threshold", () => {
  assert.deepEqual(uploadPlan(0), { mode: "single" });
  assert.deepEqual(uploadPlan(512 * 1024), { mode: "single" });
});

test("uploadPlan chooses chunked upload above the threshold", () => {
  assert.deepEqual(uploadPlan(512 * 1024 + 1), { mode: "chunked" });
});

test("effectPendingFrom reads an external approval that is waiting on the person", () => {
  const pending = effectPendingFrom({
    status: "error",
    message: "Runtime custody creator mint is pending exact Wallet or Chain settlement",
    effect_pending: {
      schema: "elastos.protected-content.effect-pending/v1",
      reason: "wallet_approval",
      awaits_person: true,
      external_signer: true,
      connector_id: "metamask",
    },
  });
  assert.deepEqual(pending, {
    reason: "wallet_approval",
    awaitsPerson: true,
    connectorId: "metamask",
  });
});

test("effectPendingFrom reads a chain wait as needing nobody", () => {
  // The distinction that matters: this one must never tell the creator to go
  // and approve something. It is already approved.
  const pending = effectPendingFrom({
    status: "error",
    message: "Runtime custody creator mint is pending exact Wallet or Chain settlement",
    effect_pending: {
      schema: "elastos.protected-content.effect-pending/v1",
      reason: "chain_settlement",
      awaits_person: false,
    },
  });
  assert.equal(pending.reason, "chain_settlement");
  assert.equal(pending.awaitsPerson, false);
  assert.equal(pending.connectorId, "");
});

test("effectPendingFrom treats a managed approval as needing nobody", () => {
  const pending = effectPendingFrom({
    effect_pending: {
      reason: "wallet_approval",
      awaits_person: false,
      external_signer: false,
      connector_id: null,
    },
  });
  assert.equal(pending.awaitsPerson, false);
  assert.equal(pending.connectorId, "");
});

test("effectPendingFrom treats an unrelated failure, and any older server, as not pending", () => {
  // A real failure carries no waiting state.
  assert.equal(
    effectPendingFrom({
      status: "error",
      message: "Runtime custody media preparation provider is unavailable",
    }),
    null,
  );
  // Absence must mean "not pending", never "unknown": an older server sends no
  // effect_pending at all, and treating that as pending would spin forever.
  assert.equal(effectPendingFrom({}), null);
  assert.equal(effectPendingFrom(undefined), null);
  assert.equal(effectPendingFrom(null), null);
  // Only a real object counts; the old detector matched a bare word anywhere
  // in the prose, which is exactly what this replaces.
  assert.equal(effectPendingFrom({ effect_pending: "pending" }), null);
  assert.equal(effectPendingFrom({ message: "something pending happened" }), null);
});

test("humanSize renders sub-kilobyte counts in bytes", () => {
  assert.equal(humanSize(0), "0 B");
  assert.equal(humanSize(1023), "1023 B");
});

test("humanSize renders sub-megabyte counts in kilobytes", () => {
  assert.equal(humanSize(1024), "1.0 KB");
  assert.equal(humanSize(1024 * 1024 - 1), "1024.0 KB");
});

test("humanSize renders megabyte-and-above counts in megabytes", () => {
  assert.equal(humanSize(1024 * 1024), "1.0 MB");
  assert.equal(humanSize(5 * 1024 * 1024), "5.0 MB");
});

test("humanSize treats a non-numeric size as zero", () => {
  assert.equal(humanSize(undefined), "0 B");
  assert.equal(humanSize(null), "0 B");
  assert.equal(humanSize("not a number"), "0 B");
});

test("resolveMime trusts the extension map for types browsers report unreliably", () => {
  assert.equal(resolveMime({ name: "book.epub", type: "" }), "application/epub+zip");
  assert.equal(resolveMime({ name: "comic.cbz", type: "application/zip" }), "application/vnd.comicbook+zip");
  assert.equal(resolveMime({ name: "model.glb", type: "" }), "model/gltf-binary");
  assert.equal(resolveMime({ name: "notes.md", type: "" }), "text/markdown");
});

test("resolveMime falls back to the browser-reported type for unmapped extensions", () => {
  assert.equal(resolveMime({ name: "clip.mp4", type: "video/mp4" }), "video/mp4");
});

test("resolveMime falls back to octet-stream when nothing else is known", () => {
  assert.equal(resolveMime({ name: "mystery", type: "" }), "application/octet-stream");
  assert.equal(resolveMime({ name: "", type: "" }), "application/octet-stream");
  assert.equal(resolveMime(undefined), "application/octet-stream");
});

test("summarizeRoyaltyRows reports ok when rows total exactly 100% (the chain's real default split)", () => {
  const result = summarizeRoyaltyRows([{ royalty: 95 }, { royalty: 5 }]);
  assert.equal(result.sum, 100);
  assert.equal(result.target, 100);
  assert.equal(result.ok, true);
  assert.equal(result.overLimit, false);
});

test("summarizeRoyaltyRows reports not-ok (but not overLimit) when the total is under 100%", () => {
  const result = summarizeRoyaltyRows([{ royalty: 50 }]);
  assert.equal(result.sum, 50);
  assert.equal(result.ok, false);
  assert.equal(result.overLimit, false);
});

test("summarizeRoyaltyRows flags overLimit distinctly when the total exceeds 100%", () => {
  const result = summarizeRoyaltyRows([{ royalty: 95 }, { royalty: 10 }]);
  assert.equal(result.sum, 105);
  assert.equal(result.ok, false);
  assert.equal(result.overLimit, true);
});

test("summarizeRoyaltyRows treats an empty or missing row list as a zero sum", () => {
  assert.equal(summarizeRoyaltyRows([]).sum, 0);
  assert.equal(summarizeRoyaltyRows(undefined).sum, 0);
  assert.equal(summarizeRoyaltyRows([{ royalty: "not a number" }]).sum, 0);
});

test("scalePriceToBaseUnits scales a whole amount at 18 decimals (ETH, ELA)", () => {
  assert.equal(scalePriceToBaseUnits("1", "ETH"), "1000000000000000000");
  assert.equal(scalePriceToBaseUnits("1", "ELA"), "1000000000000000000");
});

test("scalePriceToBaseUnits scales a fractional amount at 18 decimals", () => {
  assert.equal(scalePriceToBaseUnits("0.001", "ETH"), "1000000000000000");
});

test("scalePriceToBaseUnits scales a whole and a fractional amount at 6 decimals (USDC)", () => {
  assert.equal(scalePriceToBaseUnits("100", "USDC"), "100000000");
  assert.equal(scalePriceToBaseUnits("1.5", "USDC"), "1500000");
});

test("scalePriceToBaseUnits accepts exactly the currency's maximum fractional digits", () => {
  assert.equal(scalePriceToBaseUnits("0.123456", "USDC"), "123456");
  assert.equal(scalePriceToBaseUnits("0.123456789012345678", "ETH"), "123456789012345678");
});

test("scalePriceToBaseUnits rejects more fractional digits than the currency allows", () => {
  assert.throws(() => scalePriceToBaseUnits("0.1234567", "USDC"));
  assert.throws(() => scalePriceToBaseUnits("0.1234567890123456789", "ETH"));
});

test("scalePriceToBaseUnits rejects zero, however it is spelled", () => {
  assert.throws(() => scalePriceToBaseUnits("0", "ETH"));
  assert.throws(() => scalePriceToBaseUnits("0.000000000000000000", "ETH"));
});

test("scalePriceToBaseUnits does not lose precision on a large 18-decimal amount (no float path)", () => {
  // Number("12345678.123456789012345678") already rounds in an IEEE-754
  // double well before the 18th fractional digit; the string/BigInt path
  // must return the exact integer.
  assert.equal(
    scalePriceToBaseUnits("12345678.123456789012345678", "ETH"),
    "12345678123456789012345678",
  );
});

test("scalePriceToBaseUnits rejects an unknown currency", () => {
  assert.throws(() => scalePriceToBaseUnits("1", "DOGE"));
});

test("scalePriceToBaseUnits rejects non-numeric or negative input", () => {
  assert.throws(() => scalePriceToBaseUnits("abc", "ETH"));
  assert.throws(() => scalePriceToBaseUnits("-1", "ETH"));
  assert.throws(() => scalePriceToBaseUnits("", "ETH"));
});

test("buildPublishBody carries exactly uri, if_revision, and protection", () => {
  const body = buildPublishBody({ uri: "localhost://root/Creator/clip.mp4", ifRevision: 3, copies: "0x1", price: "0xde0b6b3a7640000" });
  assert.deepEqual(Object.keys(body).sort(), ["if_revision", "protection", "uri"]);
  assert.equal(body.uri, "localhost://root/Creator/clip.mp4");
  assert.equal(body.if_revision, 3);
});

test("buildPublishBody's protection block carries exactly mode, copies, and price", () => {
  const body = buildPublishBody({ uri: "localhost://root/Creator/clip.mp4", ifRevision: 3, copies: "0x1", price: "0xde0b6b3a7640000" });
  assert.deepEqual(Object.keys(body.protection).sort(), ["copies", "mode", "price"]);
  assert.equal(body.protection.mode, "runtime_custody");
  assert.equal(body.protection.copies, "0x1");
  assert.equal(body.protection.price, "0xde0b6b3a7640000");
});

test("buildPublishBody never carries unwired form state (title, category, royalties, ...)", () => {
  const body = buildPublishBody({
    uri: "localhost://root/Creator/clip.mp4",
    ifRevision: 1,
    copies: "0x1",
    price: "0x1",
    // Extra keys a careless caller might pass through from form state must
    // never appear in the built body — buildPublishBody only reads the four
    // named parameters above.
    title: "My asset",
    description: "What is it?",
    category: "video",
    thumbnailB64: "not-actually-sent",
    royalties: [{ address: "0xabc", royalty: 50 }],
    isAdult: true,
  });
  assert.deepEqual(Object.keys(body).sort(), ["if_revision", "protection", "uri"]);
  assert.deepEqual(Object.keys(body.protection).sort(), ["copies", "mode", "price"]);
});

// A mint that was already terminal when the request arrived raised no effect
// and sent no transaction. Reporting that as "Listed for sale" is what made a
// replay read as fresh work, so the marker has to survive the trip intact.
test("settledFrom reports a mint that was already settled before this request", () => {
  const settled = settledFrom({
    schema: "elastos.library.published-content-security/v1",
    mint_id: "620b246a",
    settled_before_this_request: true,
    settled_at: 1789479788,
    transaction_hash: "0x0994b5af",
    settled_seller_address: "0x7ba979fa244b930c01bdc84de851e5bca64b9f81",
  });
  assert.deepEqual(settled, {
    before: true,
    at: 1789479788,
    transactionHash: "0x0994b5af",
    sellerAddress: "0x7ba979fa244b930c01bdc84de851e5bca64b9f81",
  });
});

test("settledFrom treats a fresh mint, and any server without the marker, as fresh", () => {
  // A fresh mint: the server omits the marker entirely.
  assert.equal(settledFrom({ mint_id: "620b246a" }), null);
  // Absent content_security at all.
  assert.equal(settledFrom(undefined), null);
  assert.equal(settledFrom(null), null);
  // Only the exact boolean true counts. Anything else is not a claim that the
  // mint predates this request, and must not suppress the fresh-mint report.
  assert.equal(settledFrom({ settled_before_this_request: false }), null);
  assert.equal(settledFrom({ settled_before_this_request: "true" }), null);
  assert.equal(settledFrom({ settled_before_this_request: 1 }), null);
});

test("settledFrom tolerates a settled mint with no transaction hash", () => {
  const settled = settledFrom({
    settled_before_this_request: true,
    settled_at: 1789479788,
  });
  assert.equal(settled.before, true);
  assert.equal(settled.transactionHash, "");
  assert.equal(settled.sellerAddress, "");
});

// The publish stage used to be a dot labelled "(not tracked)" while the two
// around it went green — the client had no way to know, because the whole
// server pipeline ran inside one request. It is journal-derived now.
test("stagesFromProgress maps the server's phases onto this page's stages", () => {
  const mapped = stagesFromProgress({
    schema: "elastos.protected-content.creator-progress/v1",
    stages: [
      { id: "escrow", state: "done" },
      { id: "publish", state: "done" },
      { id: "listing", state: "active" },
    ],
  });
  assert.deepEqual(mapped, [
    { name: "encrypt", state: "done" },
    { name: "publish", state: "done" },
    { name: "assemble", state: "active" },
  ]);
});

test("stagesFromProgress ignores a phase this page does not render", () => {
  // A server that grows a phase must degrade to one stage fewer, never throw
  // and never invent a name for it.
  const mapped = stagesFromProgress({
    stages: [
      { id: "escrow", state: "active" },
      { id: "some-future-phase", state: "done" },
    ],
  });
  assert.deepEqual(mapped, [{ name: "encrypt", state: "active" }]);
});

test("stagesFromProgress tolerates a missing or malformed progress block", () => {
  // Absent on every server that predates it, so this must be empty rather
  // than an error.
  assert.deepEqual(stagesFromProgress(undefined), []);
  assert.deepEqual(stagesFromProgress(null), []);
  assert.deepEqual(stagesFromProgress({}), []);
  assert.deepEqual(stagesFromProgress({ stages: "escrow" }), []);
  assert.deepEqual(stagesFromProgress({ stages: [null, {}, { state: "done" }] }), []);
});

// The listing block is what turns a completed mint into a listing a
// marketplace can show. Absent it, the mint still works and publishes no
// Elacity folder — which is why absence has to be distinguishable from a
// listing whose fields are blank.
test("buildPublishBody carries listing terms when given", () => {
  const body = buildPublishBody({
    uri: "localhost://root/Creator/clip.png",
    ifRevision: 1,
    copies: "0x1",
    price: "0x1",
    listing: {
      title: "My asset",
      description: "What it is",
      category: "art",
      tags: [],
      adult: false,
      licensing: { ai_training: true },
      legal_attestation: { owns_distribution_rights: true },
    },
  });
  assert.deepEqual(Object.keys(body.protection).sort(), ["copies", "listing", "mode", "price"]);
  assert.equal(body.protection.listing.title, "My asset");
  assert.equal(body.protection.listing.licensing.ai_training, true);
});

test("buildPublishBody omits the listing entirely when there is none", () => {
  const body = buildPublishBody({
    uri: "localhost://root/Creator/clip.png",
    ifRevision: 1,
    copies: "0x1",
    price: "0x1",
  });
  assert.deepEqual(Object.keys(body.protection).sort(), ["copies", "mode", "price"]);
  assert.equal("listing" in body.protection, false);
});

// A royalty split reaches the server as ERC-1155 ROYALTY_SHARE units, which is
// the chain's own denomination: 1 unit = 0.1%, the creator's share is 950, and
// the protocol's 50 are minted by the contract and never sent as a payee.
test("buildPublishBody carries a royalty split in chain units", () => {
  const body = buildPublishBody({
    uri: "localhost://root/Creator/clip.png",
    ifRevision: 1,
    copies: "0x1",
    price: "0x1",
    listing: {
      title: "My asset",
      royalties: [
        { address: "0xab5028bdbb0826ad6f1885478e421db677b0001a", units: 900 },
        { address: "0x7ba979fa244b930c01bdc84de851e5bca64b9f81", units: 50 },
      ],
    },
  });
  const total = body.protection.listing.royalties.reduce((sum, row) => sum + row.units, 0);
  assert.equal(total, 950, "a split must come to the creator share exactly");
});

test("stagesFromProgress carries a failed stage across as the error class", () => {
  // The server gained "failed" when an aborted custody terminal stopped being
  // read as success. Without a mapping it would reach className verbatim and
  // render as nothing at all, which is how a dead mint looked fine.
  const mapped = stagesFromProgress({
    stages: [
      { id: "escrow", state: "failed" },
      { id: "publish", state: "pending" },
      { id: "listing", state: "pending" },
    ],
  });
  assert.deepEqual(mapped, [
    { name: "encrypt", state: "err" },
    { name: "publish", state: "pending" },
    { name: "assemble", state: "pending" },
  ]);
});

test("stagesFromProgress drops a state this page cannot draw", () => {
  const mapped = stagesFromProgress({
    stages: [
      { id: "escrow", state: "done" },
      { id: "publish", state: "quantum" },
    ],
  });
  assert.deepEqual(mapped, [{ name: "encrypt", state: "done" }]);
});

test("creatorMintFrom reads the typed refusal, not the sentence", () => {
  assert.deepEqual(
    creatorMintFrom({
      status: "error",
      message: "An earlier attempt to protect this file is still on record at different terms",
      creator_mint: {
        schema: "elastos.protected-content.creator-mint-blocked/v1",
        state: "recorded_only",
        mint_id: "ab12",
        recorded_copies: "3",
        recorded_price: "0.001",
        can_discard: true,
        can_resume: true,
      },
    }),
    {
      state: "recorded_only",
      mintId: "ab12",
      recordedCopies: "3",
      recordedPrice: "0.001",
      canDiscard: true,
      canResume: true,
    },
  );
});

test("creatorMintFrom returns null when the server sent no such refusal", () => {
  assert.equal(creatorMintFrom({ status: "error", message: "nope" }), null);
  assert.equal(creatorMintFrom(null), null);
});

test("failureDetailRows breaks a failure down from typed fields only", () => {
  const rows = failureDetailRows({
    message: "Runtime custody creator mint is unavailable",
    detail: "Discard the recorded terms to start over.",
    progress: {
      stages: [
        { id: "escrow", state: "done" },
        { id: "publish", state: "done" },
        { id: "listing", state: "failed" },
      ],
    },
    creatorMint: {
      state: "recorded_only",
      mintId: "ab12",
      recordedCopies: "3",
      recordedPrice: "0.001",
      canDiscard: true,
      canResume: true,
    },
  });
  assert.deepEqual(rows, [
    { label: "Reported", value: "Runtime custody creator mint is unavailable" },
    { label: "Cause", value: "Discard the recorded terms to start over." },
    {
      label: "Progress",
      value: "Encrypt & escrow: done, Publish to storage: done, Assemble listing: failed",
    },
    { label: "Existing attempt", value: "terms recorded, nothing sent to the chain" },
    { label: "Its listing id", value: "ab12" },
    { label: "Its recorded terms", value: "3 copies at 0.001" },
  ]);
});

test("failureDetailRows does not repeat the stable message as its own cause", () => {
  const rows = failureDetailRows({
    message: "Protecting this file failed.",
    detail: "Protecting this file failed.",
  });
  assert.deepEqual(rows, [{ label: "Reported", value: "Protecting this file failed." }]);
});

test("failureDetailRows names what a wait is waiting for", () => {
  assert.deepEqual(
    failureDetailRows({
      message: "pending",
      pending: { reason: "wallet_approval", awaitsPerson: true, connectorId: "metamask" },
    }),
    [
      { label: "Reported", value: "pending" },
      { label: "Waiting for", value: "a wallet approval in metamask" },
    ],
  );
  assert.deepEqual(
    failureDetailRows({
      message: "pending",
      pending: { reason: "chain_settlement", awaitsPerson: false, connectorId: "" },
    }),
    [
      { label: "Reported", value: "pending" },
      { label: "Waiting for", value: "the network to confirm the transaction" },
    ],
  );
});

test("failureDetailRows omits a field the server did not send", () => {
  assert.deepEqual(failureDetailRows({}), []);
  assert.deepEqual(failureDetailRows(null), []);
});


test("discard retries only after a receipt for the selected source", async () => {
  const calls = [];
  await discardProtectionAndRetry("localhost://owner/Creator/a.txt", async (op, body) => {
    calls.push([op, body]);
    return { schema: "elastos.library.protection-discarded/v1", uri: body.uri, discarded: true };
  }, async () => calls.push("retry"));
  assert.deepEqual(calls, [["discard_protection", { uri: "localhost://owner/Creator/a.txt" }], "retry"]);
});

test("discard refusal and malformed receipts never retry", async () => {
  let retries = 0;
  const retry = async () => { retries += 1; };
  await assert.rejects(discardProtectionAndRetry("source", async () => { throw new Error("approval outstanding"); }, retry), /approval outstanding/);
  for (const receipt of [{}, { schema: "elastos.library.protection-discarded/v1", uri: "other", discarded: true }, { schema: "elastos.library.protection-discarded/v1", uri: "source" }]) {
    await assert.rejects(discardProtectionAndRetry("source", async () => receipt, retry), /confirm/);
  }
  assert.equal(retries, 0);
});

test("already discarded receipt safely permits retry", async () => {
  let retried = false;
  await discardProtectionAndRetry("source", async () => ({ schema: "elastos.library.protection-discarded/v1", uri: "source", discarded: false }), async () => { retried = true; });
  assert.equal(retried, true);
});

// Acceptance case: a publish that takes longer than the approval budget, then a
// first approval-pending answer. The wait for the person must still get its own
// budget -- this is the defect that told a creator to approve a transaction
// which had already settled on Base.
test("a long publish does not spend the approval budget before approval begins", () => {
  const person = { awaitsPerson: true, reason: "wallet_approval", connectorId: "metamask" };
  // 310s of encrypting, uploading and pinning happened before this answer.
  const first = pendingPhase(null, person, 310_000);
  assert.equal(first.reason, "person");
  assert.equal(first.waitedMs, 0, "the approval wait starts when approval starts");
  assert.ok(first.waitedMs < pendingBudgetMs(first), "polling must not be over before it begins");
});

test("pendingPhase accumulates only the time spent in the same wait", () => {
  const person = { awaitsPerson: true };
  const first = pendingPhase(null, person, 1_000);
  const later = pendingPhase(first, person, 4_000);
  assert.equal(later.startedAt, first.startedAt);
  assert.equal(later.waitedMs, 3_000);
});

// Acceptance case: the person-to-chain transition.
test("crossing from waiting on a person to waiting on the chain starts a fresh clock", () => {
  const person = { awaitsPerson: true };
  const chain = { awaitsPerson: false, reason: "chain_settlement" };
  const waited = pendingPhase(pendingPhase(null, person, 0), person, 290_000);
  assert.equal(waited.waitedMs, 290_000, "nearly the whole person budget is spent");

  const crossed = pendingPhase(waited, chain, 290_000);
  assert.equal(crossed.reason, "chain");
  assert.equal(crossed.waitedMs, 0, "the chain wait does not inherit the person's spent time");
  assert.ok(crossed.waitedMs < pendingBudgetMs(crossed));
});

test("each wait has its own budget", () => {
  assert.equal(pendingBudgetMs({ reason: "person" }), 5 * 60 * 1000);
  assert.equal(pendingBudgetMs({ reason: "chain" }), 2 * 60 * 1000);
});

test("a wait that really does exceed its own budget still stops", () => {
  const person = { awaitsPerson: true };
  const started = pendingPhase(null, person, 0);
  const expired = pendingPhase(started, person, 5 * 60 * 1000);
  assert.ok(expired.waitedMs >= pendingBudgetMs(expired));
});

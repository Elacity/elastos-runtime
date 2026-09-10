import assert from "node:assert/strict";
import test from "node:test";

import {
  buildPublishBody,
  classifyProtection,
  decimalIntegerToHexQuantity,
  humanSize,
  isRuntimeCustodyPendingMessage,
  resolveMime,
  scalePriceToBaseUnits,
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

test("isRuntimeCustodyPendingMessage recognizes a pending Wallet or Chain settlement message", () => {
  assert.equal(
    isRuntimeCustodyPendingMessage("Runtime custody creator mint is pending exact Wallet or Chain settlement"),
    true,
  );
});

test("isRuntimeCustodyPendingMessage does not treat an unrelated failure as pending", () => {
  assert.equal(
    isRuntimeCustodyPendingMessage("Runtime custody media preparation provider is unavailable"),
    false,
  );
  assert.equal(isRuntimeCustodyPendingMessage(""), false);
  assert.equal(isRuntimeCustodyPendingMessage(undefined), false);
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

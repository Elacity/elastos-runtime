import assert from "node:assert/strict";
import {
  chmodSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  REQUIRED_ACCEPTANCE_LEGS,
  assertDistinctProfileContactEvidence,
  assertDistinctRuntimeEvidence,
  assertExactDirectConversation,
  assertFreshFixturePrecondition,
  assertFreshOwnerEnrollmentPrecondition,
  assertIdentityFrame,
  assertRecoveryBundleEvidence,
  assertRecoverySetupEvidence,
  assertRestartTransition,
  createAcceptanceReport,
  finalizeAcceptanceReport,
  loadAcceptanceConfig,
  loadRestartReceipt,
  recordAcceptancePass,
} from "./home-two-runtime-acceptance-core.mjs";

function writeOwnerOnlyJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value)}\n`, { mode: 0o600 });
  chmodSync(path, 0o600);
}

function fixtureConfig(t) {
  const root = mkdtempSync(join(tmpdir(), "elastos-acceptance-fixture-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const a = {
    fixture_id: "fixture-a",
    origin: "http://127.0.0.1:41001",
    browser_profile: join(root, "browser-a"),
    data_root: join(root, "data-a"),
    restart_receipt: join(root, "restart-a.json"),
    expected_device_did: "did:key:z6MkAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
  };
  const b = {
    fixture_id: "fixture-b",
    origin: "http://127.0.0.1:41002",
    browser_profile: join(root, "browser-b"),
    data_root: join(root, "data-b"),
    restart_receipt: join(root, "restart-b.json"),
    expected_device_did: "did:key:z6MkBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
  };
  const manifestPathA = join(root, "manifest-a.json");
  const manifestPathB = join(root, "manifest-b.json");
  const manifest = (fixture) => ({ schema: "elastos.home.acceptance-fixture/v1", ...fixture });
  const receipt = (fixture, processInstanceId) => ({
    schema: "elastos.home.acceptance-fixture-restart/v1",
    fixture_id: fixture.fixture_id,
    device_did: fixture.expected_device_did,
    process_instance_id: processInstanceId,
  });
  writeOwnerOnlyJson(manifestPathA, manifest(a));
  writeOwnerOnlyJson(manifestPathB, manifest(b));
  writeOwnerOnlyJson(a.restart_receipt, receipt(a, "process-a-before"));
  writeOwnerOnlyJson(b.restart_receipt, receipt(b, "process-b-before"));
  return {
    root,
    a,
    b,
    manifest,
    receipt,
    manifestPathA,
    manifestPathB,
    env: {
      ELASTOS_A_BASE_URL: a.origin,
      ELASTOS_A_PROFILE: a.browser_profile,
      ELASTOS_B_BASE_URL: b.origin,
      ELASTOS_B_PROFILE: b.browser_profile,
      ELASTOS_A_RESTART_CMD: "restart-fixture-a",
      ELASTOS_B_RESTART_CMD: "restart-fixture-b",
      ELASTOS_A_FIXTURE_MANIFEST: manifestPathA,
      ELASTOS_B_FIXTURE_MANIFEST: manifestPathB,
    },
  };
}

function reportConfig() {
  return {
    a: { base: "http://fixture-a.invalid", name: "Alma" },
    b: { base: "http://fixture-b.invalid", name: "Bruno" },
  };
}

test("configuration requires owner-only fixture manifests bound to distinct local Homes", (t) => {
  const fixture = fixtureConfig(t);
  const config = loadAcceptanceConfig(fixture.env);
  assert.equal(config.a.base, fixture.a.origin);
  assert.equal(config.b.profile, fixture.b.browser_profile);

  for (const key of Object.keys(fixture.env)) {
    const missing = { ...fixture.env };
    delete missing[key];
    assert.throws(() => loadAcceptanceConfig(missing), new RegExp(`${key} is required`));
  }
  assert.throws(
    () => loadAcceptanceConfig({ ...fixture.env, ELASTOS_B_BASE_URL: "https://example.com" }),
    /loopback origin/,
  );
  assert.throws(
    () => loadAcceptanceConfig({ ...fixture.env, ELASTOS_B_PROFILE: fixture.env.ELASTOS_A_PROFILE }),
    /distinct browser profile paths/,
  );

  chmodSync(fixture.manifestPathB, 0o644);
  assert.throws(() => loadAcceptanceConfig(fixture.env), /owner-only/);
  writeOwnerOnlyJson(fixture.manifestPathB, fixture.manifest(fixture.b));

  writeOwnerOnlyJson(fixture.manifestPathB, fixture.manifest({
    ...fixture.b,
    origin: fixture.a.origin,
  }));
  assert.throws(() => loadAcceptanceConfig(fixture.env), /does not bind/);
  writeOwnerOnlyJson(fixture.manifestPathB, fixture.manifest({
    ...fixture.b,
    fixture_id: fixture.a.fixture_id,
  }));
  assert.throws(() => loadAcceptanceConfig(fixture.env), /must bind distinct fixtures/);
  writeOwnerOnlyJson(fixture.manifestPathB, fixture.manifest({
    ...fixture.b,
    data_root: fixture.a.data_root,
  }));
  assert.throws(() => loadAcceptanceConfig(fixture.env), /must bind distinct fixtures/);

  writeOwnerOnlyJson(fixture.manifestPathB, fixture.manifest(fixture.b));
  const symlinkManifest = join(fixture.root, "manifest-b-link.json");
  symlinkSync(fixture.manifestPathB, symlinkManifest);
  assert.throws(
    () => loadAcceptanceConfig({ ...fixture.env, ELASTOS_B_FIXTURE_MANIFEST: symlinkManifest }),
    /fixture manifest is unavailable/,
  );

  unlinkSync(fixture.b.restart_receipt);
  assert.throws(() => loadAcceptanceConfig(fixture.env), /restart receipt is unavailable/);
  writeOwnerOnlyJson(
    fixture.b.restart_receipt,
    fixture.receipt(fixture.b, "process-b-before"),
  );
  unlinkSync(fixture.manifestPathB);
  assert.throws(() => loadAcceptanceConfig(fixture.env), /fixture manifest is unavailable/);
});

test("a loopback Home without launcher-owned fixture manifests is rejected", (t) => {
  const fixture = fixtureConfig(t);
  const nonFixture = { ...fixture.env };
  delete nonFixture.ELASTOS_A_FIXTURE_MANIFEST;
  delete nonFixture.ELASTOS_B_FIXTURE_MANIFEST;
  assert.throws(() => loadAcceptanceConfig(nonFixture), /ELASTOS_A_FIXTURE_MANIFEST is required/);
});

test("signup proof refuses a stored credential on either fixture before enrollment", () => {
  assert.doesNotThrow(() => assertFreshOwnerEnrollmentPrecondition(0, 0));
  for (const counts of [[1, 0], [0, 1], [1, 1], [undefined, 0], [0, "0"]]) {
    assert.throws(() => assertFreshOwnerEnrollmentPrecondition(...counts), /reset the issue-owned fixture Homes and browser profiles/);
  }
});

test("System evidence must prove two distinct manifest-bound Runtime instances", (t) => {
  const fixture = fixtureConfig(t);
  const config = loadAcceptanceConfig(fixture.env);
  assert.doesNotThrow(() => assertDistinctRuntimeEvidence(
    fixture.a.expected_device_did,
    fixture.b.expected_device_did,
    config,
  ));
  assert.throws(() => assertDistinctRuntimeEvidence(
    fixture.a.expected_device_did,
    fixture.a.expected_device_did,
    config,
  ), /two distinct fixture Runtimes/);
  assert.throws(() => assertDistinctRuntimeEvidence(
    "did:key:z6MkSubstituted",
    fixture.b.expected_device_did,
    config,
  ), /two distinct fixture Runtimes/);
});

test("opaque accepted-contact projections must bind distinct downloaded Recovery Profiles", () => {
  const aProfileDid = "did:key:z6MkAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
  const bProfileDid = "did:key:z6MkBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
  const aContactId = "contact:459124a94946ce899c021a75";
  const bContactId = "contact:b0785e1e57b39d59f45161a8";
  assert.deepEqual(assertDistinctProfileContactEvidence(aContactId, bContactId, aProfileDid, bProfileDid), {
    recovery_profile_binding_checked: true,
  });
  assert.throws(
    () => assertDistinctProfileContactEvidence("contact:shared", "contact:shared"),
    /distinct Profile identities/,
  );
  for (const [aDid, bDid] of [
    [undefined, bProfileDid],
    [aProfileDid, ""],
    [aProfileDid, "foreign"],
    [bProfileDid, aProfileDid],
    ["did:key:z6MkCCCCCCCCCCCCCCCCCCCCCCCCCCCC", bProfileDid],
  ]) {
    assert.throws(() => assertDistinctProfileContactEvidence(aContactId, bContactId, aDid, bDid), /bind both Recovery Profiles/);
  }
  assert.throws(
    () => assertDistinctProfileContactEvidence("", "contact:opaque-b"),
    /distinct Profile identities/,
  );
});

test("restart evidence rejects no-op, missing, malformed, and substituted receipts", (t) => {
  const fixture = fixtureConfig(t);
  const config = loadAcceptanceConfig(fixture.env);
  const before = loadRestartReceipt(config.a);
  assert.throws(() => assertRestartTransition({
    before,
    after: before,
    side: config.a,
    systemDeviceDid: fixture.a.expected_device_did,
  }), /stable-device process restart/);

  writeOwnerOnlyJson(fixture.a.restart_receipt, fixture.receipt(fixture.a, "process-a-after"));
  const after = loadRestartReceipt(config.a);
  assert.equal(
    assertRestartTransition({
      before,
      after,
      side: config.a,
      systemDeviceDid: fixture.a.expected_device_did,
    }).after_process_instance_id,
    "process-a-after",
  );

  writeOwnerOnlyJson(fixture.a.restart_receipt, fixture.receipt({
    ...fixture.a,
    fixture_id: fixture.b.fixture_id,
  }, "process-substituted"));
  assert.throws(() => loadRestartReceipt(config.a), /does not match its fixture manifest/);
  writeOwnerOnlyJson(fixture.a.restart_receipt, fixture.receipt({
    ...fixture.a,
    expected_device_did: fixture.b.expected_device_did,
  }, "process-wrong-device"));
  assert.throws(() => loadRestartReceipt(config.a), /does not match its fixture manifest/);
  writeOwnerOnlyJson(fixture.a.restart_receipt, { malformed: true });
  assert.throws(() => loadRestartReceipt(config.a), /unsupported shape/);
  writeFileSync(fixture.a.restart_receipt, "not-json\n", { mode: 0o600 });
  chmodSync(fixture.a.restart_receipt, 0o600);
  assert.throws(() => loadRestartReceipt(config.a), /restart receipt is malformed/);
  unlinkSync(fixture.a.restart_receipt);
  assert.throws(() => loadRestartReceipt(config.a), /restart receipt is unavailable/);
});

/* Report-only tests do not need fixture files; browser/runtime safety is
   exercised by the configuration tests above. */
function validReport() {
  return createAcceptanceReport(reportConfig());
}

test("pre-existing contact state fails the fresh-fixture precondition", () => {
  assert.doesNotThrow(() => assertFreshFixturePrecondition([], []));
  assert.throws(
    () => assertFreshFixturePrecondition([{ contactId: "opaque" }], []),
    /fresh fixture-owned Homes/,
  );
});

test("Welcome Recovery evidence requires the signup Profile and one fixture-owned download per side", (t) => {
  const fixture = fixtureConfig(t);
  const config = loadAcceptanceConfig(fixture.env);
  const valid = {
    a: {
      download_count: 1,
      download_path: join(fixture.a.data_root, "recovery", "fixture-a.json"),
      profile_status: "ready",
      profile_name: `${config.a.name} Admin`,
      before_recovery_status: "setup_required",
      after_recovery_status: "ready",
      principal_binding_checked: true,
      profile_included: true,
      archive_available: true,
    },
    b: {
      download_count: 1,
      download_path: join(fixture.b.data_root, "recovery", "fixture-b.json"),
      profile_status: "ready",
      profile_name: `${config.b.name} Admin`,
      before_recovery_status: "setup_required",
      after_recovery_status: "ready",
      principal_binding_checked: true,
      profile_included: true,
      archive_available: true,
    },
  };
  assert.deepEqual(assertRecoverySetupEvidence(config, valid), valid);
  assert.throws(
    () => assertRecoverySetupEvidence(config, { a: valid.a }),
    /unsupported shape/,
  );
  for (const invalid of [
    { download_count: 0 },
    { download_count: 2 },
    { profile_status: "unavailable" },
    { profile_name: "Other Profile" },
    { before_recovery_status: "ready" },
    { after_recovery_status: "unavailable" },
    { principal_binding_checked: false },
    { profile_included: false },
    { archive_available: false },
    { download_path: join(fixture.b.data_root, "recovery", "substituted.json") },
    { download_path: join(fixture.root, "outside.json") },
  ]) {
    assert.throws(
      () => assertRecoverySetupEvidence(config, { ...valid, a: { ...valid.a, ...invalid } }),
      /single verified fixture-owned Recovery setup/,
    );
  }
});

test("Recovery download must bind its data kit and signed signup Profile to the current principal", () => {
  const expected = {
    principalId: "principal:signup-a",
    localhostRoot: "localhost://Users/signup-a",
    profileName: "Alma Admin",
  };
  const aProfileDid = "did:key:z6MkAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
  const bundle = {
    schema: "elastos.full-recovery-bundle/v1",
    principal_id: expected.principalId,
    localhost_root: expected.localhostRoot,
    included: { data_kit: true, people_identity: true },
    data_kit: {
      schema: "elastos.recovery-kit/v1",
      principal_id: expected.principalId,
      localhost_root: expected.localhostRoot,
    },
    people_identity: {
      schema: "elastos.people.recovery-identity/v1",
      profile_authority_bundle: {
        schema: "elastos.profile-authority-bundle/v1",
        profile_signing_seed_hex: "11".repeat(32),
        signed_profile: {
          payload: {
            schema: "elastos.profile-document/v1",
            profile_did: aProfileDid,
            display_name: expected.profileName,
          },
          signer_did: aProfileDid,
          signature: "22".repeat(64),
        },
      },
    },
  };
  assert.deepEqual(assertRecoveryBundleEvidence(bundle, expected), {
    principal_binding_checked: true,
    profile_included: true,
  });
  for (const change of [
    (value) => { value.schema = "elastos.full-recovery-bundle.package/v1"; },
    (value) => { value.principal_id = "principal:foreign"; },
    (value) => { value.localhost_root = "localhost://Users/foreign"; },
    (value) => { value.included.data_kit = false; },
    (value) => { value.data_kit.principal_id = "principal:foreign"; },
    (value) => { value.data_kit.localhost_root = "localhost://Users/foreign"; },
    (value) => { value.data_kit.schema = "foreign"; },
  ]) {
    const invalid = structuredClone(bundle);
    change(invalid);
    assert.throws(() => assertRecoveryBundleEvidence(invalid, expected), /current principal's bound data kit/);
  }
  for (const change of [
    (value) => { delete value.people_identity; },
    (value) => { value.included.people_identity = false; },
    (value) => { value.people_identity.profile_authority_bundle.profile_signing_seed_hex = ""; },
    (value) => { value.people_identity.profile_authority_bundle.signed_profile.payload.display_name = "Foreign Profile"; },
    (value) => { value.people_identity.profile_authority_bundle.signed_profile.signer_did = "did:key:z6MkBBBBBBBBBBBBBBBBBBBBBBBBBBBB"; },
    (value) => { value.people_identity.profile_authority_bundle.signed_profile.signature = ""; },
  ]) {
    const invalid = structuredClone(bundle);
    change(invalid);
    assert.throws(() => assertRecoveryBundleEvidence(invalid, expected), /signed Profile created by signup/);
  }
  const sameNameForeignProfile = structuredClone(bundle);
  const foreignSigned = sameNameForeignProfile.people_identity.profile_authority_bundle.signed_profile;
  foreignSigned.payload.profile_did = "did:key:z6MkCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
  foreignSigned.signer_did = foreignSigned.payload.profile_did;
  // A name match alone is insufficient; final acceptance binds the download to
  // the two Runtime-accepted opaque contact projections.
  assert.doesNotThrow(() => assertRecoveryBundleEvidence(sameNameForeignProfile, expected));
  assert.throws(() => assertDistinctProfileContactEvidence(
    "contact:459124a94946ce899c021a75",
    "contact:b0785e1e57b39d59f45161a8",
    foreignSigned.payload.profile_did,
    "did:key:z6MkBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
  ), /bind both Recovery Profiles/);
});

test("only the exact opaque direct conversation can satisfy selection", () => {
  assert.doesNotThrow(() => assertExactDirectConversation({
    expectedConversationId: "conversation:opaque",
    availableConversationIds: ["shared", "conversation:opaque"],
    selectedConversationId: "conversation:opaque",
    chatMode: "direct",
  }));
  assert.throws(() => assertExactDirectConversation({
    expectedConversationId: "conversation:opaque",
    availableConversationIds: ["shared", "conversation:opaque"],
    selectedConversationId: "shared",
    chatMode: "shared",
  }), /exact accepted-contact conversation/);
  assert.throws(() => assertExactDirectConversation({
    expectedConversationId: "conversation:opaque",
    availableConversationIds: ["conversation:other"],
    selectedConversationId: "conversation:other",
    chatMode: "direct",
  }), /exact accepted-contact conversation/);
  assert.throws(() => assertExactDirectConversation({
    expectedConversationId: "conversation:opaque",
    availableConversationIds: ["conversation:opaque", "conversation:opaque"],
    selectedConversationId: "conversation:opaque",
    chatMode: "direct",
  }), /exact accepted-contact conversation/);
});

test("identity evidence rejects empty, wrong, and raw-identity frames", () => {
  const valid = {
    baseUrl: "http://127.0.0.1:41001",
    target: "people",
    frameUrl: "http://127.0.0.1:41001/apps/people/",
    text: "My Profile Alma Contacts Bruno",
  };
  assert.deepEqual(assertIdentityFrame(valid), { characters: valid.text.length });
  assert.throws(() => assertIdentityFrame({ ...valid, text: "" }), /empty frame/);
  assert.throws(
    () => assertIdentityFrame({ ...valid, frameUrl: "http://127.0.0.1:41001/apps/home/" }),
    /wrong nested frame/,
  );
  assert.throws(() => assertIdentityFrame({ ...valid, text: "Contact did:key:z6MkRaw" }), /raw or fallback identity/);
  assert.throws(() => assertIdentityFrame({ ...valid, text: "Person via remote route" }), /raw or fallback identity/);
});

test("skipped legs and incomplete reports can never become acceptance evidence", () => {
  assert.equal(REQUIRED_ACCEPTANCE_LEGS.length, 28);
  assert.deepEqual(REQUIRED_ACCEPTANCE_LEGS.slice(0, 2), ["provisioning_and_sign_in", "system_recovery_after_signup"]);
  assert.ok(REQUIRED_ACCEPTANCE_LEGS.includes("shared_session_recovery"));
  assert.ok(REQUIRED_ACCEPTANCE_LEGS.includes("direct_session_recovery"));
  // Exercise every mandatory leg, including launch, drafts, and both recovery modes.
  // A harness that omits or skips any one of them must retain ok=false.
  for (const skipped of REQUIRED_ACCEPTANCE_LEGS) {
    const report = validReport();
    for (const leg of REQUIRED_ACCEPTANCE_LEGS) {
      report.results.push({ leg, status: leg === skipped ? "skipped" : "passed" });
    }
    assert.throws(() => finalizeAcceptanceReport(report), new RegExp(`nonpassing=${skipped}`));
    assert.equal(report.ok, false);

    report.results = report.results.filter((result) => result.leg !== skipped);
    assert.throws(() => finalizeAcceptanceReport(report), new RegExp(`missing=${skipped}`));
    assert.equal(report.ok, false);
  }

  const incomplete = validReport();
  recordAcceptancePass(incomplete, REQUIRED_ACCEPTANCE_LEGS[0]);
  assert.throws(() => finalizeAcceptanceReport(incomplete), /incomplete/);
  assert.equal(incomplete.ok, false);
});

test("the final report is complete only after every required leg passed once", () => {
  const report = validReport();
  for (const leg of REQUIRED_ACCEPTANCE_LEGS) {
    recordAcceptancePass(report, leg, { observed: true });
  }
  assert.equal(finalizeAcceptanceReport(report).ok, true);
  assert.deepEqual(report.results.map((result) => result.leg), REQUIRED_ACCEPTANCE_LEGS);
  assert.throws(
    () => recordAcceptancePass(report, REQUIRED_ACCEPTANCE_LEGS[0]),
    /recorded twice/,
  );
});

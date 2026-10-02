#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const { chromium } = await import(process.env.ELASTOS_PLAYWRIGHT_MODULE
  ? pathToFileURL(process.env.ELASTOS_PLAYWRIGHT_MODULE).href : 'playwright');
const origin = process.env.ELASTOS_LEGACY_PASSKEY_URL;
const metadataPath = process.env.ELASTOS_LEGACY_PASSKEY_METADATA;
assert.ok(origin && metadataPath, 'Rust supplies an isolated gateway and fixture metadata');
let browser;
const deadline = setTimeout(() => { if (browser) void browser.close(); }, 150_000);
try {
  browser = await chromium.launch({ headless: true,
    ...(process.env.ELASTOS_CHROMIUM_EXECUTABLE ? { executablePath: process.env.ELASTOS_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext();
  context.setDefaultTimeout(10_000);
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  await cdp.send('WebAuthn.addVirtualAuthenticator', { options: {
    protocol: 'ctap2', transport: 'usb', hasResidentKey: false,
    hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true,
  } });
  // The authenticator owns its private key. This fixture never exports or
  // installs credentials through CDP; only ordinary browser ceremonies run.
  const homeResponse = await page.goto(`${origin}/home/`);
  assert.equal(homeResponse.status(), 200, 'trusted Home entry loads');
  await page.waitForFunction(() => window.legacyReady);
  const registered = await page.evaluate(async () => {
    const post = async (path, body) => {
      const response = await fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' },
        ...(body ? { body: JSON.stringify(body) } : {}) });
      if (!response.ok) {
        const raw = await response.text();
        // Keep backend reasons useful while removing values supplied in this
        // request, and long identifier/token strings, from fixture reports.
        let detail = raw;
        const redactValues = value => {
          if (typeof value === 'string' && value) detail = detail.split(value).join('[redacted]');
          else if (value && typeof value === 'object') Object.values(value).forEach(redactValues);
        };
        redactValues(body);
        detail = detail.replace(/[A-Za-z0-9_+\/=-]{16,}/g, '[redacted]').replace(/[\x00-\x1f\x7f]/g, ' ').slice(0, 300);
        throw new Error(`isolated registration refused: ${path} HTTP ${response.status}: ${detail}`);
      }
      return response.json();
    };
    const intent = { purpose: 'create', public_name: 'Legacy Fixture Owner' };
    const begin = await post('/api/auth/passkey/register/begin', { intent });
    const buffer = value => Uint8Array.from(atob(value.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
    const encode = value => btoa(String.fromCharCode(...new Uint8Array(value))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
    const options = begin.options.publicKey;
    options.challenge = buffer(options.challenge);
    options.user.id = buffer(options.user.id);
    options.excludeCredentials = [];
    // Model an already accepted older key without changing gateway completion.
    options.authenticatorSelection.residentKey = 'discouraged';
    options.authenticatorSelection.requireResidentKey = false;
    const key = await navigator.credentials.create({ publicKey: options });
    const response = await post('/api/auth/passkey/register/complete', { ceremony_id: begin.ceremony_id, intent,
      response: { id: key.id, rawId: encode(key.rawId), type: key.type,
        response: { clientDataJson: encode(key.response.clientDataJSON), attestationObject: encode(key.response.attestationObject) } } });
    return { principal_id: response.principal_id, proof_binding_id: response.proof_binding_id };
  });
  const records = () => JSON.parse(readFileSync(metadataPath, 'utf8')).principals;
  const before = records().find(record => record.principal_id === registered.principal_id);
  assert.ok(before, 'fixture registration creates a principal');
  assert.equal(before.role, 'admin');
  const hint = { schema: 'elastos.passkey.hint/v1', credential_id: before.proof_binding.passkey.credential_id,
    rp_id: before.proof_binding.passkey.rp_id };
  await page.evaluate(() => window.showLegacyUnlock());
  assert.equal(await page.evaluate(async () => (await (await fetch('/api/auth/passkey/status')).json()).guest_registration_enabled), false, 'recovery works with guest registration closed');
  await page.locator('#home-older-key-action').waitFor({ state: 'visible' });
  assert.equal(await page.evaluate(async () => {
    const begin = await (await fetch('/api/auth/passkey/authenticate/begin', { method: 'POST' })).json();
    if (begin.options.publicKey.allowCredentials.length !== 0) throw new Error('anonymous begin exposed credentials');
    const challenge = Uint8Array.from(atob(begin.options.publicKey.challenge.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
    try {
      await navigator.credentials.get({ publicKey: { ...begin.options.publicKey, challenge }, signal: AbortSignal.timeout(1200) });
      return 'unexpected-success';
    } catch (_) { return 'discovery-refused'; }
  }), 'discovery-refused', 'ordinary discovery cannot find a nonresident key');
  await page.locator('#home-older-key-action').click();
  await page.locator('#home-passkey-hint').fill(JSON.stringify({ ...hint, rp_id: 'wrong.example' }));
  await page.locator('#home-unlock-primary').click();
  await page.waitForFunction(() => document.querySelector('#home-unlock-status').dataset.tone === 'error');
  assert.equal(await page.locator('#home-passkey-hint').inputValue(), '');
  assert.equal(await page.evaluate(() => window.legacyCompletion), undefined);
  await page.locator('#home-passkey-hint').fill(JSON.stringify(hint));
  await page.locator('#home-unlock-primary').click();
  await page.waitForFunction(() => window.legacyCompletion);
  assert.equal(await page.evaluate(() => window.legacyCompletion.principal_id), registered.principal_id);
  assert.equal(await page.locator('#home-passkey-hint').inputValue(), '');
  const after = records().find(record => record.principal_id === registered.principal_id);
  assert.equal(after.localhost_root, before.localhost_root, 'sign-in preserves the existing root');
  assert.equal(after.role, before.role, 'sign-in preserves the existing Admin role');
  assert.equal(after.proof_binding_id, before.proof_binding_id, 'sign-in preserves its binding');
  // Refused proofs use the same actual gateway completion route. Errors stay
  // inside the trusted page; reports contain only the resulting status.
  const refused = await page.evaluate(async hint => {
    const buffer = value => Uint8Array.from(atob(value.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
    const encode = value => btoa(String.fromCharCode(...new Uint8Array(value))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
    const assertion = async () => {
      const begin = await (await fetch('/api/auth/passkey/authenticate/begin', { method: 'POST' })).json();
      if (begin.options.publicKey.allowCredentials.length) throw new Error('anonymous begin exposed credentials');
      const key = await navigator.credentials.get({ publicKey: { ...begin.options.publicKey,
        challenge: buffer(begin.options.publicKey.challenge), allowCredentials: [{ type: 'public-key', id: buffer(hint.credential_id) }] } });
      return { ceremony_id: begin.ceremony_id, response: { id: key.id, rawId: encode(key.rawId), type: key.type,
        response: { clientDataJson: encode(key.response.clientDataJSON), authenticatorData: encode(key.response.authenticatorData),
          signature: encode(key.response.signature), userHandle: key.response.userHandle ? encode(key.response.userHandle) : null } } };
    };
    const complete = async value => (await fetch('/api/auth/passkey/authenticate/complete', { method: 'POST',
      headers: { 'content-type': 'application/json' }, body: JSON.stringify(value) })).status;
    const invalid = await assertion();
    invalid.response.response.signature = 'AQID';
    const invalidStatus = await complete(invalid);
    // The production begin limiter allows four starts per minute. Keep this
    // complete proof matrix within that limit instead of bypassing admission.
    await new Promise(resolve => setTimeout(resolve, 60_100));
    const stale = await assertion();
    const goodStatus = await complete(stale);
    const staleStatus = await complete(stale);
    const wrongRp = await assertion();
    const authenticatorData = buffer(wrongRp.response.response.authenticatorData);
    authenticatorData[0] ^= 1;
    wrongRp.response.response.authenticatorData = encode(authenticatorData);
    const wrongRpStatus = await complete(wrongRp);
    const revoked = await assertion();
    const result = window.legacyCompletion;
    const revoke = await fetch(`/api/auth/passkeys/${encodeURIComponent(result.proof_binding_id)}/revoke`, {
      method: 'POST', headers: { 'x-elastos-home-token': result.home_token } });
    const revokedStatus = await complete(revoked);
    return { invalidStatus, goodStatus, staleStatus, wrongRpStatus, revokeStatus: revoke.status, revokedStatus };
  }, hint);
  assert.equal(refused.goodStatus, 200);
  assert.equal(refused.revokeStatus, 200);
  for (const name of ['invalidStatus', 'staleStatus', 'wrongRpStatus', 'revokedStatus']) {
    assert.ok(refused[name] >= 400, `${name} is refused`);
  }
  const final = records().find(record => record.principal_id === registered.principal_id);
  assert.equal(final.role, before.role);
  assert.equal(final.localhost_root, before.localhost_root);
  assert.ok(final.proof_binding.passkey.revoked_at);
  console.log('[legacy-passkey] PASS: nonresident recovery, same principal/root/Admin role, private begin, refused RP/stale/revoked/invalid proofs');
} finally {
  clearTimeout(deadline);
  if (browser) await browser.close();
}

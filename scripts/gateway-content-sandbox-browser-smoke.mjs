#!/usr/bin/env node
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';

const { chromium } = await import(process.env.ELASTOS_PLAYWRIGHT_MODULE
  ? pathToFileURL(process.env.ELASTOS_PLAYWRIGHT_MODULE).href : 'playwright');
const origin = process.env.ELASTOS_CONTENT_SANDBOX_URL;
const cid = process.env.ELASTOS_CONTENT_SANDBOX_CID;
assert.ok(origin && cid, 'the Rust fixture supplies its loopback gateway and CID');
const expectedPolicy = "sandbox allow-scripts allow-forms allow-popups; connect-src 'none'";
let browser;
const deadline = setTimeout(() => {
  console.error('Content sandbox browser deadline exceeded');
  if (browser) void browser.close();
}, 75_000);
try {
  browser = await chromium.launch({
    headless: true,
    ...(process.env.ELASTOS_CHROMIUM_EXECUTABLE
      ? { executablePath: process.env.ELASTOS_CHROMIUM_EXECUTABLE } : {}),
  });
  const context = await browser.newContext();
  context.setDefaultTimeout(10_000);
  context.setDefaultNavigationTimeout(10_000);
  // This signed test cookie stays in the trusted browser context. The hostile
  // document receives neither the value nor a launch token in its HTML or URL.
  await context.addCookies([{
    name: process.env.ELASTOS_CONTENT_SANDBOX_COOKIE_NAME,
    value: process.env.ELASTOS_CONTENT_SANDBOX_COOKIE_VALUE,
    url: origin,
    httpOnly: false,
    sameSite: 'Lax',
  }]);
  const page = await context.newPage();
  const apiResponses = [];
  context.on('response', response => {
    if (new URL(response.url()).pathname.startsWith('/api/')) {
      apiResponses.push({ url: response.url(), status: response.status() });
    }
  });
  const paths = [`/s/${cid}/`, `/ipfs/${cid}/`, '/'];
  for (const path of paths) {
    const responsesBeforeContent = apiResponses.length;
    await page.goto(`${origin}/home/`);
    await page.evaluate(() => {
      localStorage.setItem('runtime-sandbox-sentinel', 'trusted-home-storage');
      sessionStorage.setItem('runtime-sandbox-sentinel', 'trusted-home-session');
    });
    assert.ok(await page.evaluate(() => document.cookie.length > 0), 'trusted Home sees the test cookie');

    // A trusted gateway document shares Home's origin and storage. Use the
    // health document as parent because Home's COEP rejects untrusted frames.
    await page.goto(`${origin}/healthz`);
    // The content's own script must fail to read the trusted parent document.
    await page.evaluate(path => {
      const frame = document.createElement('iframe');
      frame.id = 'content-fixture';
      frame.src = path;
      document.body.append(frame);
    }, path);
    const handle = await page.waitForSelector('#content-fixture');
    const frame = await handle.contentFrame();
    assert.ok(frame, 'content frame exists');
    await frame.waitForFunction(() => window.probe?.ready);
    const framedProbe = await frame.evaluate(() => window.probe);
    assert.equal(framedProbe.parentDocument.denied, true, `${path}: parent DOM isolation`);
    assert.equal(framedProbe.parentStorage.denied, true, `${path}: parent storage isolation`);

    // A direct navigation also keeps the user content in an opaque origin.
    const response = await page.goto(`${origin}${path}`);
    assert.equal(response.status(), 200);
    assert.equal(response.headers()['content-security-policy'], expectedPolicy);
    await page.waitForFunction(() => window.probe?.ready);
    const probe = await page.evaluate(() => window.probe);
    assert.equal(probe.script, true, `${path}: ordinary scripts run`);
    for (const key of ['cookie', 'localStorage', 'sessionStorage']) {
      assert.equal(probe[key].denied, true, `${path}: ${key} isolation`);
    }
    assert.equal(probe.fetches.length, 5);
    assert.ok(probe.fetches.every(request => request.blocked), `${path}: Home/app fetches blocked`);
    await page.waitForFunction(() => window.probe.violations.length >= 5);
    assert.ok((await page.evaluate(() => window.probe.violations)).every(value => value === 'connect-src'));
    assert.equal(await page.locator('#result').evaluate(element => getComputedStyle(element).color), 'rgb(12, 34, 56)');
    await page.waitForFunction(() => document.querySelector('#image').naturalWidth === 8);
    await page.locator('input').fill('Ordinary form UI works');
    await page.locator('button').click();
    assert.equal(await page.locator('#result').textContent(), 'Ordinary form UI works');
    assert.equal(await page.locator('#download').getAttribute('download'), '');
    assert.equal(apiResponses.length, responsesBeforeContent, `${path}: CSP fetch probes received no API response`);

    // Forms and document navigation have separate browser channels from fetch.
    // Exercise each channel against the public summary through the real router.
    for (const channel of ['form', 'frame', 'popup']) {
      const target = `${origin}/api/apps/home/summary?sandbox_probe=${channel}`;
      const deniedResponse = context.waitForEvent('response', {
        predicate: response => response.url() === target,
      });
      if (channel === 'form') {
        await page.evaluate(target => {
          const receiver = document.createElement('iframe');
          receiver.name = 'form-api-receiver';
          document.body.append(receiver);
          const form = document.createElement('form');
          form.action = target;
          form.method = 'GET';
          form.target = receiver.name;
          const marker = document.createElement('input');
          marker.name = 'sandbox_probe';
          marker.value = 'form';
          form.append(marker);
          document.body.append(form);
          form.submit();
        }, target);
      } else if (channel === 'frame') {
        await page.evaluate(target => {
          const receiver = document.createElement('iframe');
          receiver.src = target;
          document.body.append(receiver);
        }, target);
      } else {
        const popupPromise = page.waitForEvent('popup');
        await page.evaluate(target => {
          const link = document.createElement('a');
          link.id = 'api-popup-probe';
          link.href = target;
          link.target = '_blank';
          link.textContent = 'Open API probe';
          document.body.append(link);
        }, target);
        await page.locator('#api-popup-probe').click();
        const popup = await popupPromise;
        try {
          const response = await deniedResponse;
          assert.equal(response.status(), 403, `${path}: ${channel} authority refused`);
        } finally {
          await popup.close();
        }
        continue;
      }
      assert.equal((await deniedResponse).status(), 403, `${path}: ${channel} authority refused`);
    }
    assert.equal(apiResponses.length, responsesBeforeContent + 3, `${path}: all navigation channels reached the authority gate`);
    assert.ok(apiResponses.slice(responsesBeforeContent).every(response => response.status === 403));
  }

  // Exercise the actual operator site assets under the same response policy.
  const operatorResponse = await page.goto(`${origin}/operator/`);
  assert.equal(operatorResponse.headers()['content-security-policy'], expectedPolicy);
  await page.locator('#mac-arm').click();
  assert.equal(await page.locator('#mac-arm').getAttribute('aria-selected'), 'true');
  assert.equal(await page.locator('#install-panel').getAttribute('aria-labelledby'), 'mac-arm');
  assert.ok((await page.locator('#install-guide').getAttribute('href')).endsWith('/docs/MAC.md'));
  await page.locator('#mac-arm').press('ArrowLeft');
  assert.equal(await page.locator('#linux-arm').getAttribute('aria-selected'), 'true');
  assert.ok(await page.locator('.hero-mark').evaluate(image => image.complete && image.naturalWidth > 0));
  assert.notEqual(await page.locator('html').evaluate(element => getComputedStyle(element).backgroundColor), 'rgba(0, 0, 0, 0)');
  // The production installer control is release-gated. Enable it only in this
  // browser fixture to test the existing selection recovery in the opaque page.
  await page.locator('#copy-install').evaluate(button => {
    button.disabled = false;
    document.querySelector('#install-command').closest('pre').hidden = false;
  });
  await page.locator('#copy-install').click();
  await page.waitForFunction(() => document.querySelector('#copy-status').textContent.includes('Command selected.'));
  assert.equal(await page.evaluate(() => window.getSelection().toString()),
    await page.locator('#install-command').textContent());
  console.log('PASS: real gateway CID/site sandbox, Home/app isolation, content UI, and operator site recovery');
} finally {
  clearTimeout(deadline);
  if (browser) await browser.close();
}

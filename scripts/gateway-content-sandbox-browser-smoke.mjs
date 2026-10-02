#!/usr/bin/env node
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
import { createServer } from 'node:http';

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
    // The front-door fixture copies the real Home modules. Use its trusted
    // gateway origin here to seed storage without starting a signed desktop.
    await page.goto(`${origin}${process.env.ELASTOS_FRONTDOOR_FIXTURE === '1' ? '/healthz' : '/home/'}`);
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
  if (process.env.ELASTOS_FRONTDOOR_FIXTURE === '1') {
    await checkFrontdoor(browser, context, page);
  }
  console.log('PASS: real gateway CID/site sandbox, Home/app isolation, content UI, and operator site recovery');
} finally {
  clearTimeout(deadline);
  if (browser) await browser.close();
}

async function checkFrontdoor(browser, context, page) {
  const token = process.env.ELASTOS_FRONTDOOR_APP_TOKEN;
  assert.ok(token, 'the fixture supplies an app-scoped launch');
  const assertAppDocument = async (documentPage, path) => {
    await documentPage.waitForFunction(() => window.appProbe?.ready);
    const probe = await documentPage.evaluate(() => window.appProbe);
    assert.equal(probe.script, true, `${path}: external script runs under response CSP`);
    assert.equal(probe.reads.cookie.denied, true, `${path}: Home cookie stays private`);
    assert.equal(probe.reads.storage.denied, true, `${path}: Home storage stays private`);
    const compilation = probe.compilation;
    assert.equal(compilation.wasm, true, `${path}: existing app WebAssembly compilation works`);
    assert.equal(compilation.javascript, 'EvalError', `${path}: JavaScript string compilation stays refused`);
    const etag = await documentPage.evaluate(async () => {
      const response = await fetch('/apps/assistant/extra.svg?v=browser-fixture', { credentials: 'omit' });
      return response.headers.get('etag');
    });
    assert.match(etag || '', /^"[a-f0-9]{64}"$/, `${path}: the opaque client can read its revision header`);
    const denied = await documentPage.evaluate(() => window.runAppRequests(''));
    assert.ok(denied.every(result => result.status === 403 || result.failed === 'TypeError'),
      `${path}: an unsigned app document has no API authority`);
    const wire = await observeWireResponses(documentPage.context(), documentPage);
    const refusedHome = wire.response(`${origin}/api/apps/home/summary`);
    const authorized = await documentPage.evaluate(token => window.runAppRequests(token), token);
    assert.equal(authorized[0].status, 200, `${path}: signed app header fetch works`);
    assert.equal((await refusedHome).status, 403, `${path}: app launch keeps Home authority private`);
    assert.ok(authorized[1].status === 403 || authorized[1].failed === 'TypeError');
    for (const header of ['x-elastos-upload-offset', 'x-elastos-recovery-terminal', 'if-match', 'if-none-match', 'range']) {
      const result = await documentPage.evaluate(async ({ token, header }) => {
        const response = await fetch('/api/apps/assistant/workspace', {
          credentials: 'omit', headers: { 'x-elastos-home-token': token, [header]: 'fixture' },
        });
        return response.status;
      }, { token, header });
      assert.equal(result, 200, `${path}: the existing ${header} protocol survives opaque preflight`);
    }
  };
  for (const path of ['/apps/assistant/', '/apps/assistant/extra.html', '/apps/assistant/extra.svg']) {
    const response = await page.goto(`${origin}${path}`);
    assert.equal(response.status(), 200, `${path}: document loads`);
    const policy = response.headers()['content-security-policy'];
    assert.equal(response.headers()['cross-origin-opener-policy'], 'same-origin',
      `${path}: the app keeps its existing opener isolation policy`);
    assert.ok(policy?.includes('sandbox allow-scripts'), `${path}: document has a response sandbox`);
    assert.ok(!policy.includes('allow-same-origin'), `${path}: sandbox gives an opaque origin`);
    assert.ok(policy.includes("script-src 'self'"), `${path}: scripts keep the document source boundary`);
    await assertAppDocument(page, path);
  }
  await page.goto(`${origin}/apps/assistant/`);
  const failedPopupRequests = [];
  const popupApiRequests = [];
  const recordFailure = request => {
    if (new URL(request.url()).pathname === '/apps/assistant/extra.html') {
      failedPopupRequests.push(request.failure()?.errorText);
    }
  };
  const recordApiRequest = request => {
    if (new URL(request.url()).pathname.startsWith('/api/')) popupApiRequests.push(request.url());
  };
  context.on('requestfailed', recordFailure);
  context.on('request', recordApiRequest);
  const popupPromise = page.waitForEvent('popup');
  await page.locator('#escape').click();
  const popup = await popupPromise;
  try {
    await popup.waitForLoadState('domcontentloaded');
    if (popup.url() === 'chrome-error://chromewebdata/') {
      // An inherited sandbox cannot navigate a popup to a COOP same-origin
      // document. Chromium refuses the response before app scripts can run.
      assert.deepEqual(failedPopupRequests, ['net::ERR_BLOCKED_BY_RESPONSE'],
        'the attempted app popup is refused by response policy');
      assert.equal(await popup.evaluate(() => Boolean(window.appProbe)), false,
        'the refused popup never executes the app probe');
      assert.equal(await popup.locator('body').innerText(), '',
        'the refused popup displays no app or Home data');
      assert.deepEqual(popupApiRequests, [], 'the refused popup sends no Home or app API request');
    } else {
      assert.equal(popup.url(), `${origin}/apps/assistant/extra.html`);
      await assertAppDocument(popup, 'app popup');
    }
  } finally {
    context.off('requestfailed', recordFailure);
    context.off('request', recordApiRequest);
    await popup.close();
  }

  // A separate browser context starts with the real Home assets and no grant.
  const anonymous = await browser.newContext();
  try {
    const home = await anonymous.newPage();
    const unsignedChatRequests = [];
    const recordUnsignedChatRequest = request => {
      const path = new URL(request.url()).pathname;
      if (path.startsWith('/api/') || path.endsWith('.wasm')) unsignedChatRequests.push(path);
    };
    home.on('request', recordUnsignedChatRequest);
    const chatResponse = await home.goto(`${origin}/apps/chat-room/?home_origin=https%3A%2F%2Fforeign.example`);
    assert.equal(chatResponse.status(), 200);
    const chatPolicy = chatResponse.headers()['content-security-policy'];
    assert.ok(chatPolicy?.startsWith('sandbox ') && !chatPolicy.includes('allow-same-origin'),
      'direct Chat keeps an opaque response sandbox');
    assert.equal(await home.locator('#chat-open-home').isVisible(), true);
    assert.equal(await home.locator('#chat-open-home').getAttribute('href'), '/home/');
    assert.equal(await home.locator('#chat-open-home').getAttribute('target'), '_self');
    assert.equal(await home.locator('#browser-access-form').count(), 0);
    assert.match(await home.locator('#browser-access-stage').innerText(), /Sign in to Home, then open Chat\./);
    assert.deepEqual(unsignedChatRequests, [], 'unsigned Chat starts neither WASM nor private API requests');
    home.off('request', recordUnsignedChatRequest);
    const unsignedSummary = await anonymous.request.get(`${origin}/api/apps/chat-room/summary`);
    assert.equal(unsignedSummary.status(), 403, 'anonymous Chat summary remains private');
    const unsignedJoin = await anonymous.request.post(`${origin}/api/browser/session/request`, {
      data: { display_name: 'Recovery fixture', capabilities: ['room.access'] },
    });
    assert.equal(unsignedJoin.status(), 403, 'anonymous guest admission remains refused');
    await Promise.all([
      home.waitForURL(`${origin}/home/`),
      home.locator('#chat-open-home').click(),
    ]);
    assert.equal(anonymous.pages().length, 1, 'Open Home recovers in the same window');
    await home.waitForFunction(() => {
      const unlock = document.querySelector('#home-unlock');
      return unlock && !unlock.hidden && ['#home-unlock-primary', '#home-unlock-person'].some(selector => {
        const button = document.querySelector(selector);
        return button && !button.disabled && button.getClientRects().length > 0;
      });
    });
    assert.equal(await home.locator('#home-shell-boot-mask').evaluate(element => element.hidden), true);
    assert.match(await home.locator('#home-unlock-title').textContent(), /Sign in|Set up Home|Resume setup/);
    assert.equal(await home.locator('#shell-host-recovery').evaluate(element => element.hidden), true,
      'anonymous API refusal opens the Home sign-in controls');
  } finally {
    await anonymous.close();
  }

  const bootstrapPaths = ['/api/carrier/bootstrap', '/.well-known/elastos/carrier-bootstrap.json'];
  // Give this disposable attacker permission to reach loopback so the Runtime
  // gate, rather than Chromium's local-network prompt, decides the request.
  const attacker = await browser.newContext({ permissions: ['local-network-access'] });
  const server = createServer((_request, response) => {
    response.writeHead(200, { 'content-type': 'text/html' });
    response.end('<!doctype html><title>Other localhost origin</title>');
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  try {
    const hostile = await attacker.newPage();
    await hostile.route('https://hostile.example/**', route => route.fulfill({
      contentType: 'text/html', body: '<!doctype html><title>Internet attacker</title><iframe id="opaque" sandbox="allow-scripts" srcdoc="<!doctype html><title>Opaque attacker</title>"></iframe>',
    }));
    await hostile.goto('https://hostile.example/');
    const traffic = await observeWireResponses(attacker, hostile);
    const frame = await (await hostile.waitForSelector('#opaque')).contentFrame();
    assert.ok(frame);
    const attemptBootstrap = async (source, label, expectedOrigin) => {
      for (const path of bootstrapPaths) {
        const url = `${origin}${path}?browser_frontdoor_probe=${encodeURIComponent(label)}`;
        const responsePromise = traffic.response(url);
        const fetchPromise = source.evaluate(async ({ url, forged }) => {
          try {
            const response = await fetch(url, { credentials: 'include', headers: { Origin: forged } });
            return { status: response.status, body: await response.text() };
          } catch (error) { return { failed: error.name }; }
        }, { url, forged: origin });
        const response = await responsePromise;
        assert.equal(response.status, 403, `${label}: bootstrap is refused by the real gateway`);
        assert.equal(response.origin, expectedOrigin, `${label}: browser owns the Origin header`);
        const result = await fetchPromise;
        assert.ok(result.status === 403 || result.failed === 'TypeError');
        assert.doesNotMatch(result.body || '', /did:[a-z0-9]+:/i, `${label}: browser receives no DID`);
        const navigation = await attacker.newPage();
        try {
          const deniedDocument = await navigation.goto(url, {
            referer: expectedOrigin === 'null' ? 'https://hostile.example/' : `${expectedOrigin}/`,
          });
          assert.equal(deniedDocument.status(), 403, `${label}: bootstrap document navigation is refused`);
          assert.doesNotMatch(await navigation.locator('body').innerText(), /did:[a-z0-9]+:/i,
            `${label}: refused document keeps DID private`);
        } finally {
          await navigation.close();
        }
      }
    };
    await attemptBootstrap(frame, 'internet-opaque', 'null');
    const otherOrigin = `http://127.0.0.1:${server.address().port}`;
    await hostile.goto(otherOrigin);
    await attemptBootstrap(hostile, 'other-localhost-port', otherOrigin);
    for (const path of ['/api/carrier/bootstrap', '/api/apps/home/summary', '/home/']) {
      const response = await context.request.get(`${origin}${path}`, {
        headers: { Host: 'rebound.attacker.example', Origin: origin },
      });
      assert.equal(response.status(), 403, `${path}: DNS rebinding Host is refused over HTTP`);
      assert.doesNotMatch(await response.text(), /did:[a-z0-9]+:/i);
    }
  } finally {
    await attacker.close();
    await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
  console.log('PASS: gateway app document/popup isolation, signed app fetch, anonymous Home entry, bootstrap Origin and Host admission');
}

async function observeWireResponses(context, page) {
  const session = await context.newCDPSession(page);
  await session.send('Network.enable');
  const records = new Map();
  const waiters = new Set();
  const update = (id, fields) => {
    records.set(id, { ...records.get(id), ...fields });
    for (const waiter of waiters) waiter();
  };
  session.on('Network.requestWillBeSent', event => update(event.requestId, { url: event.request.url, method: event.request.method }));
  session.on('Network.requestWillBeSentExtraInfo', event => {
    const origin = Object.entries(event.headers).find(([name]) => name.toLowerCase() === 'origin')?.[1];
    update(event.requestId, { origin });
  });
  session.on('Network.responseReceivedExtraInfo', event => update(event.requestId, { status: event.statusCode }));
  return {
    response(url) {
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          waiters.delete(check);
          reject(new Error('Browser wire response deadline exceeded'));
        }, 10_000);
        const check = () => {
          const result = [...records.values()].find(record => record.url === url && record.method === 'GET' && record.status && record.origin);
          if (!result) return;
          clearTimeout(timer);
          waiters.delete(check);
          resolve(result);
        };
        waiters.add(check);
        check();
      });
    },
  };
}

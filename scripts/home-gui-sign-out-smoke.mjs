#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";

const root = new URL("../", import.meta.url);
const indexHtml = fs.readFileSync(
  new URL("capsules/home-gui/browser/index.html", root),
  "utf8",
);
const templateHtml = fs.readFileSync(
  new URL("capsules/home-gui/browser/home-gui-template.html", root),
  "utf8",
);
const shellSource = fs.readFileSync(
  new URL("capsules/home-gui/browser/home-gui-shell.js", root),
  "utf8",
);
const chromeSource = fs.readFileSync(
  new URL("capsules/home-gui/browser/shell-chrome.js", root),
  "utf8",
);
const {
  isTrustedHomeGuiMessage,
  projectHomeGuiAuthority,
} = await import(
  new URL(
    "capsules/home-gui/browser/home-gui-authority.js",
    root,
  )
);

assert.doesNotMatch(
  indexHtml.match(/<body\b[^>]*>/)?.[0] || "",
  /data-home-authority=/,
  "Home GUI must start without projected authority",
);
assert.match(
  templateHtml,
  /id="identity-menu-sign-out"[^>]*\shidden(?:\s|>)/,
  "Home GUI must hide sign-out in markup before trusted state arrives",
);
assert.match(
  chromeSource,
  /const signedIn = summary\?\.authority\?\.signed_in === true;/,
  "Home chrome must use exact signed truth for sign-out visibility",
);
assert.match(
  chromeSource,
  /if \(toolbarSignOutButton\) \{\s*toolbarSignOutButton\.hidden = false;\s*\}/s,
  "Home chrome must show sign-out only after exact signed identity sync",
);
assert.match(
  chromeSource,
  /if \(toolbarSignOutButton\) \{\s*toolbarSignOutButton\.hidden = true;\s*\}/s,
  "Home chrome must hide sign-out when identity clears",
);

const body = { dataset: {} };
projectHomeGuiAuthority(body, { authority: { signed_in: true } });
assert.equal(body.dataset.homeAuthority, "signed");
for (const summary of [
  null,
  {},
  { authority: {} },
  { authority: { signed_in: false } },
  { authority: { signed_in: "true" } },
]) {
  projectHomeGuiAuthority(body, summary);
  assert.equal(
    body.dataset.homeAuthority,
    "unsigned",
    "only exact signed_in=true may project signed authority",
  );
}

const trustedParent = {};
assert.equal(
  isTrustedHomeGuiMessage(
    { source: trustedParent, origin: "http://localhost:61180" },
    trustedParent,
    "http://localhost:61180",
  ),
  true,
);
assert.equal(
  isTrustedHomeGuiMessage(
    { source: {}, origin: "http://localhost:61180" },
    trustedParent,
    "http://localhost:61180",
  ),
  false,
  "a forged source must not be trusted",
);
assert.equal(
  isTrustedHomeGuiMessage(
    { source: trustedParent, origin: "http://evil.invalid" },
    trustedParent,
    "http://localhost:61180",
  ),
  false,
  "a forged origin must not be trusted",
);

const projectionIndex = shellSource.indexOf("projectHomeGuiAuthority(document.body, summary);");
const firstAwaitIndex = shellSource.indexOf("await syncHomeGuiProjection", projectionIndex);
assert.ok(projectionIndex >= 0, "Home GUI shell must project trusted summary authority");
assert.ok(
  firstAwaitIndex > projectionIndex,
  "Home GUI shell must project authority before awaiting other summary work",
);
assert.match(shellSource, /signOut:\s*\(\)\s*=>\s*requestHome\("home:sign-out"\)/);
assert.doesNotMatch(
  shellSource,
  /\/api\/auth\/sessions\/sign-out/,
  "isolated Home GUI must not revoke Runtime sessions directly",
);

console.log("[home-gui-sign-out] PASS checks=17");

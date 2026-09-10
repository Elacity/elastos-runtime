#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync, existsSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, resolve, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { validHostedReceipt } from "./website-claims.mjs";
const HOME_PATH = "/home/";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const site = resolve(repo, "website/elastos");
const read = (path) => readFileSync(resolve(site, path), "utf8");
const html = read("index.html");
const css = read("site.css");
const facts = JSON.parse(read("claims.json"));
const checks = [];
function check(name, run) {
  try { run(); checks.push(name); }
  catch (error) { throw new Error(`${name}: ${error.message}`, { cause: error }); }
}

check("facts identify source and dated evidence", () => {
  assert.equal(facts.schema, "elastos.website.claims/v1");
  assert.match(facts.reviewed_at, /^\d{4}-\d{2}-\d{2}$/);
  assert.match(facts.candidate.base_ref, /^upstream\/.+-dev$/);
  assert.match(facts.candidate.base_commit, /^[a-f0-9]{40}$/);
  assert.ok(facts.candidate.evidence.includes(`/${facts.candidate.base_commit}/state.md`));
  assert.match(facts.public_observation.observed_at, /^\d{4}-\d{2}-\d{2}$/);
  for (const key of ["release_head_sha256", "release_sha256", "installer_sha256"]) assert.match(facts.public_observation[key], /^[a-f0-9]{64}$/);
  assert.deepEqual(facts.public_observation.evidence, ["/release-head.json", "/release.json", "/install.sh"]);
  if (facts.hosted.status === "verified") assert.ok(validHostedReceipt(facts.hosted, new URL(facts.hosted.target).origin), "hosted version needs a dated deployment receipt");
  else assert.equal(facts.hosted.status, "unverified");
});

check("visitor content stays useful without release services", () => {
  assert.ok(html.includes('href="#install"'), "visitors need a device installation path");
  assert.ok(html.includes('src="./site.js"'), "platform selection and copy use a local script");
  assert.ok(html.includes('id="copy-install" disabled'), "keep copying unavailable until 0.7.1 installation is verified");
  assert.ok(html.includes("0.7.1 installer coming soon."));
  assert.ok(html.includes('aria-label="Installation command" hidden'), "reveal command only with the verified installer");
  assert.ok(!/0\.1\.2|Mac download in preparation|guest access is open/.test(html));
  assert.ok(!/xcode-select|ELASTOS_SOURCE_HOME|git clone/.test(html), "source build steps belong in the developer guide");
  assert.ok(html.includes("Development preview."));
  assert.ok(html.includes("sign in with a passkey"));
  assert.ok(html.includes("Recovery Kit"));
  assert.ok(html.includes("For developers"));
  assert.ok(!/Version awaiting verification|Release proof open|Device proof open|See the evidence|Publisher DID|Installer SHA-256/i.test(html));
});

const endpoints = new Set(["/", HOME_PATH, "/install.sh", "/release-head.json", "/release.json", "/claims.json"]);
const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]);
const slug = (text) => text.toLowerCase().replace(/[^\p{L}\p{N}\s-]/gu, "").replace(/\s/g, "-");
const pinnedFiles = new Map();
function checkLink(value, asset = false) {
  if (value.startsWith("https://")) {
    assert.equal(asset, false, `external asset ${value}`);
    const url = new URL(value);
    assert.equal(url.hostname, "github.com");
    assert.ok(url.pathname.startsWith("/Elacity/elastos-runtime"));
    const pinned = url.pathname.match(/^\/Elacity\/elastos-runtime\/blob\/([a-f0-9]{40})\/(.+)$/);
    if (pinned) {
      assert.equal(pinned[1], facts.candidate.base_commit);
      if (!pinnedFiles.has(pinned[2])) pinnedFiles.set(pinned[2], execFileSync("git", ["show", `${pinned[1]}:${pinned[2]}`], { cwd: repo, encoding: "utf8" }));
      if (url.hash) {
        const headings = [...pinnedFiles.get(pinned[2]).matchAll(/^#{1,6}\s+(.+)$/gm)].map((match) => slug(match[1]));
        assert.ok(headings.includes(decodeURIComponent(url.hash.slice(1))), `missing source anchor ${value}`);
      }
    }
    return;
  }
  assert.ok(!/^(?:[a-z]+:|\/\/)/i.test(value), `unexpected URL ${value}`);
  if (value.startsWith("#")) return assert.ok(ids.includes(value.slice(1)), `missing page anchor ${value}`);
  if (value.startsWith("/")) return assert.ok(endpoints.has(value), `unknown gateway route ${value}`);
  const path = resolve(site, value);
  assert.ok(!relative(site, path).startsWith(".."), `asset escapes site: ${value}`);
  assert.ok(existsSync(path), `missing asset ${value}`);
}

check("page and evidence links resolve", () => {
  assert.equal(ids.length, new Set(ids).size, "duplicate HTML id");
  for (const match of html.matchAll(/\b(href|src)="([^"]+)"/g)) checkLink(match[2], match[1] === "src");
  for (const match of css.matchAll(/url\(["']?([^"')]+)["']?\)/g)) checkLink(match[1], true);
  checkLink(facts.candidate.evidence);
  for (const journey of facts.journeys) checkLink(journey.evidence);
  if (facts.hosted.evidence) checkLink(facts.hosted.evidence);
});

check("the primary action opens Home on this server", () => {
  assert.match(html, new RegExp(`id="open-home" href="${HOME_PATH}"`));
  assert.ok(!html.includes('href="http://localhost'), "local source examples stay separate from the primary link");
});

check("copy and claims describe evidence honestly", () => {
  const text = html.replace(/<[^>]+>/g, " ");
  assert.ok(!/PC2|chat --nick|md-viewer|works today|first 10 minutes|latest version|verified download|signature verified/i.test(text));
  assert.ok(!/class="verified"|✓/.test(html));
  assert.ok(!/\/Users\/|\/private\/tmp\/|\.ssh\//.test(html + JSON.stringify(facts)), "private operator detail in public source");
  assert.ok(!/data:image|unpkg\.com|fonts\.google|react-dom/.test(html + css));
});

check("assets have source parity", () => {
  const pairs = [
    ["elastos-logo.svg", "capsules/home/browser/elastos-logo.svg"],
    ["elastos-mark.svg", "capsules/home/browser/elastos-home-icon.svg"],
    ["Inter-latin-var.woff2", "capsules/home/browser/assets/fonts/Inter-latin-var.woff2"],
    ...["documents", "library", "people", "system"].map((app) => [`${app}.png`, `capsules/${app}/browser/icons/icon-128.png`]),
  ];
  for (const [asset, source] of pairs) assert.ok(readFileSync(resolve(site, "assets", asset)).equals(readFileSync(resolve(repo, source))), `asset differs from source: ${asset}`);
  assert.deepEqual(readdirSync(resolve(site, "assets")).sort(), pairs.map(([asset]) => asset).sort());
});

console.log(`[website-truth-check] pass (${checks.length} checks)`);

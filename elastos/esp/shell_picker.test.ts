/**
 * ESP v0 — shell-picker + refraction conformance tests.
 *
 * Proves: only role-`shell` + launchable capsules are selectable (a non-shell can
 * never become the active shell — the fail-closed mirror of the runtime's
 * `shell_token_eligible`); the picker renders honest Trust Cards; and a refraction
 * toggle preserves the projected state across a focus-swap.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { ESP_SCHEMA_TAGS } from "./esp_v0.js";
import type { CapsuleCatalogResponse, CapsuleSummary } from "./esp_v0.js";
import {
  isShellSelectable,
  selectableShells,
  shellPicker,
  shellTrustCard,
  withActiveShell,
} from "./shell_picker.js";
import { makeRefraction, toggleFocus } from "./refraction.js";

function cap(
  p: Partial<CapsuleSummary> & Pick<CapsuleSummary, "name" | "role" | "launchable">,
): CapsuleSummary {
  return {
    version: "0.1.0",
    title: p.name,
    description: "",
    type: "wasm",
    category: "app",
    installed: true,
    trust_state: "local-dev",
    signature_state: "no-manifest-signature",
    cid_state: "local-only",
    ...p,
  };
}

const catalog: CapsuleCatalogResponse = {
  schema: ESP_SCHEMA_TAGS.capsuleCatalog,
  capsules: [
    cap({ name: "shell", role: "shell", launchable: true, trust_state: "local-manifest-signature" }),
    cap({ name: "flint", role: "shell", launchable: true, trust_state: "cid-with-manifest-signature" }),
    cap({ name: "halfshell", role: "shell", launchable: false }), // shell but not launchable
    cap({ name: "marketplace", role: "app", launchable: true }), // an app — never a shell
  ],
};

describe("shell selectability (fail-closed, mirrors shell_token_eligible)", () => {
  it("a shell-role launchable capsule is selectable", () => {
    assert.ok(isShellSelectable({ role: "shell", launchable: true }));
  });
  it("a shell that is not launchable is NOT selectable", () => {
    assert.ok(!isShellSelectable({ role: "shell", launchable: false }));
  });
  it("a non-shell capsule is NEVER selectable, even if launchable", () => {
    assert.ok(!isShellSelectable({ role: "app", launchable: true }));
  });
  it("selectableShells returns only the eligible shells", () => {
    const names = selectableShells(catalog).map((c) => c.name);
    assert.deepEqual(names, ["shell", "flint"]);
  });
});

describe("the shell-picker", () => {
  it("defaults the active shell to the first selectable one", () => {
    assert.equal(shellPicker(catalog).active, "shell");
  });
  it("honours a caller-provided active shell when it is selectable", () => {
    assert.equal(shellPicker(catalog, "flint").active, "flint");
  });
  it("falls back when the requested active shell is not selectable", () => {
    // "marketplace" is an app — the picker refuses it and falls back fail-closed.
    assert.equal(shellPicker(catalog, "marketplace").active, "shell");
  });
  it("withActiveShell switches to a selectable shell and refuses others", () => {
    const picker = shellPicker(catalog);
    assert.equal(withActiveShell(picker, "flint")?.active, "flint");
    assert.equal(withActiveShell(picker, "marketplace"), null, "a non-shell can never become active");
  });
  it("renders an honest Trust Card from the shell's own verdict", () => {
    const flint = catalog.capsules.find((c) => c.name === "flint")!;
    assert.deepEqual(shellTrustCard(flint), { name: "flint", title: "flint", trust: "verified" });
  });
});

describe("the refraction toggle", () => {
  it("swaps focus while carrying the projected state through unchanged", () => {
    const projected = { catalogSchema: catalog.schema, items: 4 };
    const start = makeRefraction("shell", "flint", projected);
    assert.equal(start.focused, "shell");

    const swapped = toggleFocus(start);
    assert.equal(swapped.focused, "flint");
    // One source of authority, NO state migration: the projected state is identical.
    assert.strictEqual(swapped.projected, projected);

    // Toggling again returns to the original lens.
    assert.equal(toggleFocus(swapped).focused, "shell");
  });
});

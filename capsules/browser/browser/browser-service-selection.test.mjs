import assert from "node:assert/strict";
import test from "node:test";
import { engineChoiceHidden, renderServiceSelection } from "./browser-service-selection.js";

function selectElement() {
  return {
    options: [],
    ownerDocument: { createElement: () => ({}) },
    replaceChildren(...options) { this.options = options; },
    add(option) { this.options.push(option); },
    set value(id) {
      this.selected = this.options.find((option) => option.value === id)?.value || "";
    },
    get value() { return this.selected; },
  };
}

for (const service of ["Engine", "Exit"]) {
  test(`${service} intent survives disappearance and return of an approved offer`, () => {
    const select = selectElement();
    const selected = { id: "approved-service", label: "Chosen service" };
    const other = { id: "another-service", label: "Another service" };
    const options = {
      selectedId: selected.id,
      defaultLabel: "Default",
      unavailableLabel: `Selected ${service} (unavailable)`,
      labelForService: (entry) => entry.label,
    };
    for (const services of [[selected, other], [], [other], [other, selected]]) {
      const result = renderServiceSelection(select, { ...options, services });
      assert.equal(select.value, selected.id);
      assert.equal(result, services.includes(selected) ? selected : null);
      assert.equal(select.options.filter((item) => item.value === selected.id).length, 1);
    }
    // Changing to the default is an explicit operator action.
    renderServiceSelection(select, { ...options, selectedId: "", services: [selected] });
    assert.equal(select.value, "");
    assert.equal(select.options.length, 2);
  });
}

test("automatic selection stays with Runtime as the inventory changes", () => {
  const select = selectElement();
  for (const services of [[], [{ id: "engine-one" }], [{ id: "engine-two" }]]) {
    renderServiceSelection(select, {
      services, selectedId: "", defaultLabel: "Automatic",
      labelForService: (entry) => entry.id, unavailableLabel: "Unavailable",
    });
    assert.equal(select.value, "");
  }
});

test("one engine that Automatic uses is not offered as a choice", () => {
  const local = { id: "browser-vm-product", backing_substrate: "local_microvm" };
  assert.equal(engineChoiceHidden([local], ""), true);
  assert.equal(engineChoiceHidden([], ""), true);
  assert.equal(engineChoiceHidden([local], "browser-vm-product"), false, "an explicit choice stays visible");
  assert.equal(engineChoiceHidden([{ id: "mac-remote", backing_substrate: "remote_operator_vm" }], ""), false,
    "Automatic skips remote VMs, so the remote engine stays choosable");
  assert.equal(engineChoiceHidden([{ id: "remote-engine-aa", backing_substrate: "local_microvm" }], ""), false,
    "a granted remote service is never picked by Automatic, so it stays choosable");
  assert.equal(engineChoiceHidden([local, { id: "jetson", backing_substrate: "local_microvm" }], ""), false);
});

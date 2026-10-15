// Discovery updates availability. Only the operator changes service intent.
export function renderServiceSelection(select, {
  services,
  selectedId,
  defaultLabel,
  labelForService,
  unavailableLabel,
}) {
  const option = (label, value) => {
    const element = select.ownerDocument.createElement("option");
    element.textContent = label;
    element.value = value;
    return element;
  };
  const selected = services.find((service) => service.id === selectedId) || null;
  select.replaceChildren(option(defaultLabel, ""));
  for (const service of services) {
    select.add(option(labelForService(service), service.id));
  }
  if (selectedId && !selected) {
    select.add(option(unavailableLabel, selectedId));
  }
  select.value = selectedId;
  return selected;
}

// One engine that Automatic already uses is not a choice, so Settings hides it.
// Automatic never picks a remote VM or a granted remote service (remote-engine-*
// selection ids, gateway_browser_remote.rs), so those stay choosable.
export function engineChoiceHidden(engines, selectedId) {
  const lone = engines[0];
  return !selectedId && engines.length <= 1 && (!lone ||
    (lone.backing_substrate !== "remote_operator_vm" && !String(lone.id).startsWith("remote-engine-")));
}

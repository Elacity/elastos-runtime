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

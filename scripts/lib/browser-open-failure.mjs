// Retain the stage of non-JSON admission failures without retaining response text.
export async function browserOpenResponseEvidence(response) {
  const body = await response.json().catch(() => null);
  if (body !== null) return { body, response_format: "json" };
  const text = await response.text().catch(() => "");
  const known = new Map([
    ["Browser lifecycle already owns a different open intent", "different_open_intent"],
    ["Browser instance already owns an active or in-flight open", "instance_open_in_flight"],
    ["Browser instance already owns an active or launching lifecycle", "instance_lifecycle_active"],
  ]);
  return { body: null, response_format: "non_json",
    admission_reason: known.get(text.trim()) || "unclassified_non_json" };
}

import type { CapsuleSummary, InspectObjectProjection, JsonValue } from "./esp_v0.ts";

export type TrustMaterial = "verified" | "content_addressed" | "unsigned";

export function trustMaterial(capsule: Pick<CapsuleSummary, "trust_state">): TrustMaterial {
  switch (capsule.trust_state) {
    case "cid-with-manifest-signature":
    case "local-manifest-signature":
      return "verified";
    case "cid-without-manifest-signature":
      return "content_addressed";
    default:
      return "unsigned";
  }
}

export interface ProvenanceView {
  state: "absent" | "signed" | "unsigned" | "incomplete";
  author: JsonValue;
  cid: string | null;
  signature_present: boolean;
  signature_fingerprint: string | null;
  signer_known: boolean;
}

export function provenanceView(
  object: Pick<InspectObjectProjection, "provenance"> | null | undefined,
): ProvenanceView {
  const provenance = object?.provenance;
  if (!provenance) {
    return {
      state: "absent",
      author: null,
      cid: null,
      signature_present: false,
      signature_fingerprint: null,
      signer_known: false,
    };
  }
  const signature_present = provenance.signature_present === true;
  const signature_fingerprint =
    typeof provenance.signature_fingerprint === "string"
      ? provenance.signature_fingerprint
      : null;
  const state = signature_present
    ? signature_fingerprint
      ? "signed"
      : "incomplete"
    : "unsigned";
  return {
    state,
    author: provenance.author ?? null,
    cid: typeof provenance.cid === "string" ? provenance.cid : null,
    signature_present,
    signature_fingerprint,
    signer_known: typeof provenance.signed_by === "string" && provenance.signed_by.length > 0,
  };
}

export interface TrustCard {
  name: string;
  title: string;
  trust: TrustMaterial;
  provenance?: ProvenanceView;
}

export function trustCard(
  capsule: Pick<CapsuleSummary, "name" | "title" | "trust_state">,
  object?: Pick<InspectObjectProjection, "provenance"> | null,
): TrustCard {
  return {
    name: capsule.name,
    title: capsule.title,
    trust: trustMaterial(capsule),
    ...(object ? { provenance: provenanceView(object) } : {}),
  };
}

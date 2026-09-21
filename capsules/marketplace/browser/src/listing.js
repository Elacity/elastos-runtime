// The protected-item listing contract, as Runtime publishes it.
//
// This module is the consumer half of one written agreement. Its producer is
// `runtime_custody_listing_summary` and `runtime_custody_listing_availability`
// in elastos/crates/elastos-server/src/protected_content_runtime.rs, and the
// two halves drifted apart once already: the producer gained an availability
// `receipt_digest` after this parser pinned its key set, and every row Runtime
// published from that day on was refused by the exact-key check. One malformed
// row used to take the whole shelf down with it, so the failure read as
// "Couldn't load" rather than as one item the app declined to show.
//
// Two rules follow from that history. A field the producer sends is named here
// even when nothing renders it, so a new field is a decision rather than an
// outage. And a row that fails validation is dropped alone: the shelf still
// renders, and the caller learns how many rows it refused.
//
// The values themselves stay exactly as Runtime sent them. A quantity or a
// price is a uint256 in hex; it becomes a decimal string through
// `uint256Decimal` at the moment it is displayed and never a JavaScript number.

export const RUNTIME_CUSTODY_LISTINGS_RESPONSE_SCHEMA_V1 = "elastos.library.runtime-custody-listings/v1";
export const RUNTIME_CUSTODY_LISTING_SCHEMA_V1 = "elastos.library.runtime-custody-listing/v1";
export const RUNTIME_CUSTODY_AVAILABILITY_SCHEMA_V1 = "elastos.library.runtime-custody-availability-summary/v1";
export const MAX_RUNTIME_CUSTODY_LISTINGS = 128;
export const MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES = 256;
export const RUNTIME_CUSTODY_MINT_ID = /^[0-9a-f]{64}$/;

const MAX_U32 = 0xffffffff;
const MAX_SAFE_TIMESTAMP = Number.MAX_SAFE_INTEGER;
const UINT256_HEX = /^0x(?:0|[1-9a-f][0-9a-f]{0,63})$/;
const ADDRESS_HEX = /^0x[0-9a-f]{40}$/;
const SHA256_HEX = /^[0-9a-f]{64}$/;
// `elastos://` and a plain content identifier, which is what Runtime writes
// and the only spelling its own origin check accepts. Both CID encodings in
// use are alphanumeric, so a separator here means a path, a query or a
// fragment -- none of which name a listing.
const LISTING_URI = /^elastos:\/\/[A-Za-z0-9]{16,128}$/;
const ACCESS_STATES = ["available", "creator", "purchased"];

// The two kinds Runtime publishes, spelled as it spells them
// (RUNTIME_CUSTODY_VIEWER_CONTENT_KIND_MEDIA and its object twin). The same
// two words arrive on the viewer session, so a row and the session it opens
// agree by construction rather than by a rule written twice.
export const CONTENT_KIND_MEDIA = "media";
export const CONTENT_KIND_OBJECT = "object";
const CONTENT_KINDS = [CONTENT_KIND_MEDIA, CONTENT_KIND_OBJECT];

// Every key of one listing row, and of the availability summary inside it, in
// the producer's own spelling. Listed rather than inferred so that adding a
// field to Runtime and forgetting it here fails a test in this file instead of
// emptying the shelf in front of a person.
const LISTING_KEYS = [
  "access_state",
  "availability",
  "codecs",
  "content_kind",
  "display_name",
  "listing_uri",
  "mime_type",
  "mint_id",
  "pay_token",
  "price",
  "published_at",
  "quantity",
  "schema",
  "seller_address",
  "token_id",
];
const AVAILABILITY_KEYS = [
  "checked_at",
  "observed_replicas",
  "receipt_digest",
  "recheck_before_buy",
  "recheck_before_open",
  "required_replicas",
  "schema",
  "status",
];

/**
 * Parse the answer to `list_runtime_custody`.
 *
 * Returns the rows that validated, whether Runtime truncated its own list, and
 * how many rows this parser refused. A caller that finds `rejected` above zero
 * has something worth saying to the person; it does not have a reason to hide
 * the rows that are sound.
 */
export function parseRuntimeCustodyListings(payload) {
  assertExactKeys(payload, ["schema", "listings", "truncated"]);
  if (payload.schema !== RUNTIME_CUSTODY_LISTINGS_RESPONSE_SCHEMA_V1) {
    throw new Error("invalid protected item schema");
  }
  if (!Array.isArray(payload.listings) || payload.listings.length > MAX_RUNTIME_CUSTODY_LISTINGS) {
    throw new Error("invalid protected item list");
  }
  if (typeof payload.truncated !== "boolean") {
    throw new Error("invalid protected item truncation state");
  }
  const listings = [];
  let rejected = 0;
  for (const entry of payload.listings) {
    try {
      listings.push(parseRuntimeCustodyListing(entry));
    } catch (error) {
      // The row is the unit of trust. Refusing it protects the person from
      // acting on a record this app cannot read, and refusing only it keeps
      // the rest of their shelf in front of them.
      rejected += 1;
      reportRejectedListing(entry, error);
    }
  }
  return { listings, truncated: payload.truncated, rejected };
}

/** Parse one listing row. All of it validates, or none of it is returned. */
export function parseRuntimeCustodyListing(value) {
  assertExactKeys(value, LISTING_KEYS);
  if (value.schema !== RUNTIME_CUSTODY_LISTING_SCHEMA_V1) {
    throw new Error("invalid protected item entry");
  }
  const accessState = boundedString(value.access_state, 16);
  if (!ACCESS_STATES.includes(accessState)) {
    throw new Error("invalid protected item access state");
  }
  const contentKind = boundedString(value.content_kind, 16);
  if (!CONTENT_KINDS.includes(contentKind)) {
    throw new Error("invalid protected item kind");
  }
  const mintId = boundedString(value.mint_id, 64);
  // The address this listing's package lives at, which is what one person
  // gives another. It is checked here with the same rule the add-a-listing
  // field uses, so a link this app would refuse is never a link it offers.
  const listingUri = listingUriFromInput(value.listing_uri);
  if (!listingUri) {
    throw new Error("invalid protected item link");
  }
  const displayName = boundedString(value.display_name, MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES);
  const mimeType = boundedString(value.mime_type, MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES);
  // An object has no codecs and Runtime sends the empty string for it, so this
  // is the one public text field where empty is the producer's own answer.
  const codecs = boundedText(value.codecs, MAX_RUNTIME_CUSTODY_PUBLIC_TEXT_BYTES);
  const quantity = boundedString(value.quantity, 66);
  const price = boundedString(value.price, 66);
  const payToken = boundedString(value.pay_token, 42);
  const publishedAt = boundedTimestamp(value.published_at);
  const sellerAddress = boundedString(value.seller_address, 42);
  const tokenId = boundedString(value.token_id, 66);
  if (!RUNTIME_CUSTODY_MINT_ID.test(mintId)) {
    throw new Error("invalid protected item mint");
  }
  if (!UINT256_HEX.test(quantity) || !UINT256_HEX.test(price)) {
    throw new Error("invalid protected item quantity");
  }
  if (!ADDRESS_HEX.test(payToken) || !ADDRESS_HEX.test(sellerAddress)) {
    throw new Error("invalid protected item address");
  }
  if (!UINT256_HEX.test(tokenId)) {
    throw new Error("invalid protected item token");
  }
  return {
    accessState,
    availability: parseAvailabilitySummary(value.availability),
    codecs,
    contentKind,
    displayName,
    listingUri,
    mimeType,
    mintId,
    payToken,
    price,
    publishedAt,
    quantity,
    sellerAddress,
    tokenId,
  };
}

/**
 * Parse the availability summary carried by one row.
 *
 * `receipt_digest` names the signed availability receipt the replica count was
 * read from. It is validated and then left behind: a row shows how many
 * replicas answered, and the digest belongs to the evidence trail rather than
 * to the shelf.
 */
export function parseAvailabilitySummary(value) {
  assertExactKeys(value, AVAILABILITY_KEYS);
  if (value.schema !== RUNTIME_CUSTODY_AVAILABILITY_SCHEMA_V1 || value.status !== "last_verified_receipt") {
    throw new Error("invalid protected item availability");
  }
  if (!SHA256_HEX.test(String(value.receipt_digest ?? ""))) {
    throw new Error("invalid protected item availability receipt");
  }
  const checkedAt = boundedTimestamp(value.checked_at);
  const requiredReplicas = boundedCount(value.required_replicas);
  const observedReplicas = boundedCount(value.observed_replicas);
  if (
    requiredReplicas === 0
    || observedReplicas < requiredReplicas
    || value.recheck_before_buy !== true
    || value.recheck_before_open !== true
  ) {
    throw new Error("invalid protected item availability state");
  }
  return {
    checkedAt,
    observedReplicas,
    requiredReplicas,
  };
}

/**
 * The viewer one listed item needs.
 *
 * Runtime answers this in the row itself. Its open path admits exactly one
 * viewer per kind — `expected_runtime_custody_viewer_capsule` matches media to
 * the player and an object to the reader, with no arm that admits both — and
 * `content_kind` is that same decision, read from the content identity. An app
 * that guessed the kind from the MIME instead was keeping a second rule for a
 * question already answered, and a second rule can disagree: Marketplace used
 * to send every item to the player, which is how a picture arrived at a video
 * surface.
 *
 * Anything without a kind this app knows goes to Reader, which renders an
 * honest unsupported state, rather than to Player, which would show a blank
 * frame.
 */
export function viewerForListing(listing) {
  return listing?.contentKind === CONTENT_KIND_MEDIA ? "elacity-player" : "elacity-reader";
}

/**
 * The listing link a person pasted, or the empty string if it is not one.
 *
 * Runtime writes `elastos://<cid>` and accepts nothing else: a creator listing
 * whose URI is not exactly that is refused. This checks the shape only — the
 * prefix, a plain content identifier, no path, query or fragment — and leaves
 * the identifier itself to Runtime, which parses the CID, fetches the package,
 * and verifies its metadata, its chain record and its content before the
 * listing reaches anyone's shelf. Repeating that judgement here would be a
 * second opinion with less evidence.
 */
export function listingUriFromInput(value) {
  const text = String(value ?? "").trim();
  return LISTING_URI.test(text) ? text : "";
}

// The stages Runtime reports while a purchase settles, and what each one means
// for the person in front of the screen. Spelled as Runtime spells them, from
// `elastos.protected-content.buy-progress/v1`.
export const BUY_PROGRESS_SCHEMA_V1 = "elastos.protected-content.buy-progress/v1";
const BUY_STAGES = [
  "allowance_approval",
  "purchase_approval",
  "chain_settlement",
  "access_evidence",
  "declined",
];

/**
 * What the app should do next, read from a buy answer that did not succeed.
 *
 * A purchase is a sequence of waits: the wallet holds an approval, then the
 * network confirms what was approved, then the right becomes readable. All
 * three used to arrive as one sentence that this app filtered away, so a
 * purchase in progress looked exactly like one that had failed. The answer now
 * names its stage, and this turns that into the three things worth doing:
 * keep waiting, say whose turn it is, or stop.
 *
 * An answer that carries no progress, or one whose progress does not validate,
 * is a failure rather than a wait. Waiting on a state this app cannot read
 * would leave a person watching a spinner that no answer will ever end.
 */
export function buyOutcomeFromAnswer(answer) {
  const progress = answer && typeof answer === "object" ? answer.buy_progress : null;
  if (
    !progress
    || typeof progress !== "object"
    || progress.schema !== BUY_PROGRESS_SCHEMA_V1
    || !BUY_STAGES.includes(progress.stage)
    || typeof progress.resumable !== "boolean"
    || typeof progress.awaits_person !== "boolean"
  ) {
    return { kind: "failed", stage: "", awaitsPerson: false, connectorId: "" };
  }
  const connectorId = typeof progress.connector_id === "string" ? progress.connector_id : "";
  if (!progress.resumable) {
    return {
      kind: progress.stage === "declined" ? "declined" : "failed",
      stage: progress.stage,
      awaitsPerson: false,
      connectorId,
    };
  }
  return {
    kind: "waiting",
    stage: progress.stage,
    awaitsPerson: progress.awaits_person,
    connectorId,
  };
}

/** A uint256 as Runtime sends it, shown in full decimal. Never a float. */
export function uint256Decimal(value) {
  if (!UINT256_HEX.test(value)) {
    throw new Error("invalid protected item field");
  }
  return BigInt(value).toString(10);
}

function boundedString(value, maxBytes) {
  if (!value) {
    throw new Error("invalid protected item field");
  }
  return boundedText(value, maxBytes);
}

function boundedText(value, maxBytes) {
  if (
    typeof value !== "string"
    || new TextEncoder().encode(value).length > maxBytes
    || /[ -]/.test(value)
  ) {
    throw new Error("invalid protected item field");
  }
  return value;
}

function boundedCount(value) {
  if (!Number.isInteger(value) || value < 0 || value > MAX_U32) {
    throw new Error("invalid protected item count");
  }
  return value;
}

function boundedTimestamp(value) {
  if (!Number.isSafeInteger(value) || value <= 0 || value > MAX_SAFE_TIMESTAMP) {
    throw new Error("invalid protected item timestamp");
  }
  return value;
}

function assertExactKeys(value, keys) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("invalid protected item object");
  }
  const actual = Object.keys(value).sort().join("\n");
  const expected = [...keys].sort().join("\n");
  if (actual !== expected) {
    throw new Error("invalid protected item shape");
  }
}

// A refused row is worth one line an operator can act on: which mint, and what
// was wrong with it. The row's own values stay out of it — a record this parser
// could not read is not a record it should quote back into the log.
function reportRejectedListing(entry, error) {
  const mintId = entry && typeof entry === "object" ? String(entry.mint_id || "") : "";
  const named = RUNTIME_CUSTODY_MINT_ID.test(mintId) ? mintId : "unnamed";
  console.warn(`marketplace refused a protected item listing (${named}): ${error.message}`);
}

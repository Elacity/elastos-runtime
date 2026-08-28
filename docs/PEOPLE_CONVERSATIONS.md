# People, Contacts, and Conversations

This document describes the implemented collaboration candidate and the later
work that is intentionally outside it.

Current branch state:

- Runtime owns the principal-scoped signed Profile, contact store, conversation
  state, collaboration workers, routing, lifecycle, and audit;
- signed discovery, Inbox-owned contact approval, direct Chat, Profile updates,
  bilateral contact removal, re-add, shared-room Profile attribution, restart
  continuity, and recovery are implemented;
- People, Chat, and Inbox use typed Runtime resources and do not select Carrier
  peers, routes, tickets, ports, or provider topology;
- the configured shared conversation is the first group implementation. It is
  not a global room or a template for every future group.

Current acceptance gap:

- the exact candidate is installed on localhost, but the normal localhost
  one-Runtime product acceptance is not complete;
- the public seed still needs the same exact candidate before the real
  two-Runtime product journey can run;
- an explicit alternate signed network profile selects a separate network, and
  missing configuration selects isolation;
- the seed/profile signer is bootstrap/config authority only, never
  person/contact/message authority.

Later work outside this candidate:

- wider discovery rendezvous and abuse controls;
- encrypted mailbox delivery for people who remain offline;
- user-created groups with signed membership and durable catch-up;
- multi-device pairing, silent block, and direct-message attachments;
- isolation of the old Services remote-Exit social/contact path from People
  identity and contact authority.

## Goal

Make it easy for people using ElastOS to find each other, approve trust, and
chat/share objects without exposing runtime DIDs, provider names, or transport
details in the main UI.

The first useful product outcome is simple:

- I can choose a display name for my Profile.
- I can choose whether other people can find this Home.
- I can send or accept a contact request.
- I can open Chat and see conversations, not runtime plumbing.
- I can share a document/object into a conversation and open it in ElastOS.

## Core Decision

Do not create separate `contacts`, `profile`, and `conversations` WASM capsules
yet. That is likely correct later, but it is too much surface area before the
object model and UX are proven.

For the near term:

- `Profile` is a compact People "My Profile" surface backed by Runtime-owned
  ProfileAuthority.
- `People` is a Home section backed by contact objects.
- `Conversations` is a Chat Room module/view backed by conversation objects.
- `Inbox` remains the approval surface for contact, conversation, and
  capability requests.

Split these into separate capsules only after the user-visible flow is stable
and the provider/object contracts are small enough to review.

## Ontology

Use these concepts consistently:

- `Principal`: the local authority subject unlocked by a passkey.
- `ProfileAuthority`: one protected principal-root bundle containing the
  profile signing secret and the current signed public Profile document. It
  must use protected-only storage and reject an unprotected principal root.
- `Profile`: the signed public identity document. It contains `profile_did`,
  display name, optional handle, monotonic revision, previous-profile hash,
  update time, and the currently authorized delivery device bindings.
- `Device DID`: a routing and signing identity for one Runtime device. It is
  not the human contact identity.
- `DeviceBinding`: an authorization inside the signed Profile document that
  links one current device DID to that Profile DID. A Profile authorizes
  exactly one device: the wire format carries a list, but the product path
  always writes the current local device, so update delivery covers renaming
  and carrying a data root (or recovering) to another machine — never a
  second concurrent device.
- `DiscoveryAdvertisement`: a short-lived signed publication of the current
  signed Profile document while discovery is on and this Home is online.
- `ContactRequest`: an Inbox-reviewed request bound to one exact signed
  discovery advertisement.
- `Contact`: a mutual relationship derived from a verified request plus the
  exact signed acceptance receipt.
- `Conversation`: a direct, group, or public thread between profile identities.
- `Message`: a signed object in a conversation.
- `Attachment`: a byte-backed Chat attachment or object handle shared into a
  shared conversation. Direct conversations are text-only for now.

Passkey, Principal, Profile DID, and Device DID are separate on purpose:

- passkey = local authorization proof;
- principal = local authority subject;
- Profile DID = stable contact/conversation identity;
- device DID = transport/signing endpoint.

Runtime/device DIDs are allowed as internal transport/signing material, but
they are not the product identity shown to users.

For this candidate, a Profile has one locally managed delivery endpoint. The
document can represent more bindings, but multi-device profile lifecycle and
sync are later work; normal UI shows neither device labels nor device DIDs.

## UX Model

### People / My Profile

User-facing actions:

- Edit my profile card.
- Turn Discovery on or off.

Copy rules:

- Say "device", "profile", "people", "conversation".
- Do not say "runtime DID" in ordinary UI.
- If a DID is shown in advanced details, label it as technical proof material.

### People / Contacts

User-facing actions:

- See accepted contacts.
- Choose whether other people can find this Home.
- See the bounded `Visible now` list of people who have opted in, with optional
  local filtering.
- Send contact request.
- Start chat.
- Remove contact.

Discovery must be opt-in, bounded, and honest:

- off by default;
- advertisements renew only while the user has turned Discovery on and this
  Home is online; after a disconnect or failed withdrawal, the last
  advertisement can remain visible for at most its existing TTL;
- `Visible now` is bounded and never implies a global roster or directory;
- turning Discovery off stops new advertisement refresh and withdraws the
  current advertisement when transport is available.

### Inbox

Inbox owns approval:

- `Alice wants to add you as a contact`.
- `Alice invited you to a conversation`.

Each request needs clear Accept/Decline actions and enough context to decide. Do
not ask for broad permission before the user initiates or receives a concrete
request.

### Chat / Conversations

Chat should show conversations:

- `Global` later, if/when public global chat is intentionally added.
- Direct conversations with contacts.
- Group conversations.
- The current managed/shared room can remain as the first group-conversation
  implementation, but the UI should present participants, not owners/admins.

Near-term UI copy should avoid defaulting to "owner/admin/member" unless the
user opens advanced room controls. The normal participant list should show
signed Profile names only, for example `Alex`; device labels and DIDs remain
routing or diagnostic detail.

## UI Patterns

### Profile Card

Use one compact card everywhere:

- avatar/color mark
- display name
- short handle or profile proof badge when available
- relationship state: `You`, `Contact`, `Requested`, or `Pending`
- primary action: `Message`, `Add contact`, `Accept`, `Decline`, or `Remove`

Do not show raw DID text on the card. Put technical identifiers behind
`Details`.

### Discovery Panel

Discovery should feel simple and intentional:

- big toggle: `Turn Discovery On`
- clear `Turn Off` action
- bounded `Visible now` list with optional local filtering
- discovered profile cards in a simple grid/list
- empty state: `No one is visible right now`

The panel should never imply background permanent visibility, a universal
directory, or unique global names.

### Inbox Request Card

Every request card should answer:

- who is asking
- what they want
- what accepting allows
- what data will be shared

Example:

`Alice wants to add you as a contact. Accepting shares your profile card and lets
Alice start a direct conversation with you.`

Actions: `Accept`, `Decline`.

People may show that requests are waiting in Inbox, but it should not become a
second approval surface.

### Chat Conversation Layout

Keep Chat familiar:

- left rail: conversations (`Global` later, direct chats, group rooms)
- main pane: messages and attachments
- right/details pane: participants, shared objects, conversation settings

For the current branch, this can be visually simulated with the current
single-room UI: rename the current room to a conversation, make participants
readable, and keep advanced room controls out of the normal send/read path.

### Mobile / Small Screen

Use one pane at a time:

- Conversations list
- Message thread
- Details

The primary action should remain reachable without exposing advanced controls.

## Discovery Flow

Discovery is convenience, not the only path.

1. User saves a real display name and turns Discovery on.
2. Runtime publishes a short-lived signed `DiscoveryAdvertisement` derived from
   the current signed Profile document.
3. Other opt-in Homes can show the bounded `Visible now` list, with optional
   local filtering.
4. User clicks `Add contact`.
5. Recipient receives an Inbox `ContactRequest`.
6. Accept creates a mutual contact on both sides and allows Chat to start a
   direct conversation keyed by the two Profile DIDs.

Privacy constraints:

- Off by default.
- User can stop immediately.
- `Visible now` is bounded.
- The signed public Profile document is deliberately minimal.
- Use opaque advertisement ids in UI; avoid stable tracking identifiers beyond
  the signed Profile DID already needed for contact authority.
- Discovery advertisements refresh while enabled and online; after disconnect
  or failed withdrawal, the last advertisement can remain visible only until
  its existing TTL expires.

## QR, Code, and Link Flow

QR/code/link remains an explicit alternate onboarding and invite path and
should ship first.

Use it for:

- Add a contact directly when both sides already intend to connect.
- Invite someone to a conversation.
- Join a shared room from another runtime.

The invite payload should contain:

- invitation kind: contact request or conversation invite
- inviter profile card summary
- room/conversation id when relevant
- Carrier bootstrap/rendezvous material
- short-lived signed secret
- expiry

It should not require the sender to type the receiver's DID before sharing the
invite. The receiver introduces its profile/device proof during accept.

## Implementation Slices

The six source slices below are implemented on the unpublished collaboration
branch. The normal localhost and public seed installation checks remain release
gates, not source behavior.

### Slice 1 - Copy and Mental Model Cleanup

Status: implemented in-repo. Normal UI renders signed Profile names or an
explicit unverified marker. Placeholder and device-name fallbacks are gone.

Scope:

- Keep current Chat behavior.
- Change visible labels away from owner/runtime language where possible.
- Keep advanced room controls available but not central.
- Keep `scripts/chat-room-live-roster-smoke.sh` as the non-mutating roster
  proof.

User test:

- Open Chat on public Linux and Mac.
- Confirm participant labels make sense.
- Send text both ways, and a document attachment both ways where the room
  serves attachments (the plain shared room; the configured room refuses them
  explicitly).
- Confirm no runtime DID/owner/admin wording appears in the normal path.

### Slice 2 - Join Shared Conversation by Code

Status: implemented. Signed peer invite-link transport and Profile-backed
group participant attribution are both in place.

Scope:

- Add a simple invite-code/link flow over the existing managed-room backend.
- Receiver accepts without manually entering a DID.
- Keep invite-link transport independent of the pending Profile-backed group
  participant attribution work.

User test:

- Server creates invite code.
- Mac joins using code.
- Both sides see each other and exchange messages, plus attachments where the
  room serves them.

### Slice 3 - Profile authority bundle

Status: implemented and signed end to end, including recipient-side retention
of the verified remote Profile head and signed Profile update delivery across
runtimes, proven live in
[COLLABORATION_HANDOFF.md](COLLABORATION_HANDOFF.md) §1. The Full Recovery
Bundle carries the Profile signing seed, revision ring, and contact store, so
recovery onto a fresh machine keeps the Profile DID, authorizes the new
device through the normal signed-revision path, and accepted contacts learn
the new endpoint from one announcement — proven live in
`recovered_identity_rebinds_a_fresh_device_and_contacts_learn_it`.

Scope:

- Add one protected principal-root ProfileAuthority bundle.
- Keep the signing secret private under the principal root.
- Reject profile-secret creation when the principal root is not protected.
- Expose one signed public Profile document with revision and authorized device
  bindings.
- Keep profile creation/update explicit and passkey-authorized.

User test:

- Save a display name with a proof-bound Home session.
- Runtime creates the protected profile authority once.
- Reads create no profile secret or device identity.
- Recipients retain the highest accepted profile revision and reject older or
  conflicting revisions.

### Slice 4 - Opt-in Discovery, durable outbox, Inbox approval, and contact store

Status: implemented. Signed advertisements, requests, decisions, and the durable
contact store are in place; Inbox is the only Accept/Decline surface.

Scope:

- Publish/read bounded signed discovery advertisements.
- Require a real user-entered display name before enabling Discovery or sending
  a request.
- Send a contact request bound to one exact advertisement.
- Project incoming requests into Inbox and accept/decline there only.
- Derive accepted contacts from signed request + acceptance chains keyed by
  remote Profile DID.
- Use the signed local store as the outbox so relay loss/restart cannot strand
  a sent request or acceptance.

User test:

- Turn Discovery on from one runtime.
- Refresh `Visible now` from the other runtime.
- Send request, accept in Inbox, and see the accepted contact authority land on
  both sides with no second user click after relay loss/restart.

### Slice 5 - People/Contacts read model

Status: implemented. The read model carries the complete relationship states —
connected, requested, declined, removed, removed_you — plus reachability, all
projected from signed chains.

Scope:

- Add a minimal People view backed by accepted contact chains.
- Key accepted contacts by remote Profile DID.
- Surface the stable direct conversation id, but keep direct chat disabled until
  the transport slice lands.
- Keep Inbox as the approval surface.

The source also covers decline, explicit empty or unavailable state when
Profile authority is absent, and separation from the Services peer-contact
store. It does not fall back to device-contact truth.

User test:

- Accepted contacts appear in People by signed display name.
- Requested state is visible without duplicate clicks.
- Reading People creates no identity, device key, or profile side effects.

### Slice 6 - Direct chat

Status: implemented. Direct conversations are Profile-scoped, Runtime-authorized
on every read and send, and routed over the Runtime-mediated peer path. Shared
group Chat is unchanged. Bilateral removal, shared-room Profile attribution,
and incoming-message notifications have landed with it. Direct conversations
are text-only for now: attachments need object handling and a delivery path of
their own, designed on the unified delivery layer, and until then the attach
control is visibly unavailable in direct mode rather than silently missing.

Scope:

- Keep the shared group conversation unchanged.
- Launch the same Chat capsule in direct mode from a People contact using only
  an opaque conversation id.
- Derive direct conversation identity from the two Profile DIDs.
- Deliver direct messages over Runtime-mediated point-to-point Carrier routing,
  never the shared gossip conversation.
- Verify direct messages in two layers:
  - device signature proves the sending endpoint;
  - the retained signed Profile document proves that device is currently
    authorized for the participant Profile DID.
- The selected Carrier session already provides encrypted peer confidentiality:
  the current `PeerDid` path uses Iroh QUIC/TLS, relays forward encrypted bytes
  only, and Runtime binds delivery to the accepted Profile's current authorized
  endpoint before admitting the message. The seed/bootstrap path receives no
  direct-message plaintext and no direct-message authority.
- Plaintext direct-message content exists only in the sender's and recipient's
  protected principal-root direct-message store under
  `.AppData/ElastOS/Chat/direct-messages.json`.

User test:

- Two accepted contacts exchange one direct message each way.
- Group chat remains unchanged.
- Removing a contact blocks future direct sends/receives.
- A signed-in Home without a saved Profile must show one clear `Create your
  Profile` action instead of entering signed group or direct Chat under a
  device, principal, or guest fallback. Profile save remains explicit and
  passkey-authorized; public guest invites remain a separate mode.

## Offline Reach Limits

Both conversation transports have a stated reach limit for a peer that stays
offline, and neither invents a third party to hide it:

- Direct messages: the sender's Runtime durably retries until the envelope's
  lifetime ends, then the message is terminal and visibly `expired`. There is
  no store-and-forward; the encrypted mailbox design in TASKS.md is the
  deliberate path to more reach.
- The shared room: gossip topic buffers are in-memory on whichever peer holds
  them, and consumer cursors are what let a returning peer collect what it
  missed. A peer offline past that buffer's retention, or across a restart of
  the peer holding it, misses that interval; whatever does arrive is ingested
  durably. This is a transport reach limit, not a storage bug, and it is the
  shared-room half of the same offline gap.
- Profile updates: the announcement chain segment retains the last 8 signed
  revisions. A contact more than 8 renames behind cannot be bridged; the
  receiving store refuses the gap explicitly ("accepted profile chain skips a
  revision") and the pair needs a fresh approval — the same explicit decision
  reserved for total device loss, decided in
  [COLLABORATION_HANDOFF.md](COLLABORATION_HANDOFF.md) §1. The window is a
  deliberate bound, not a shared constant with the 64-head contact store: the
  ring bounds announcement payloads, the head store bounds relationships.

## Near-Term Non-Goals

- Attachments in direct conversations. Deferred by decision, not omission:
  attachment bytes need object handling, retention, and a delivery path of
  their own, and that path is designed on the unified delivery layer together
  with the encrypted mailbox rather than as another ad-hoc transport. The UI
  states the deferral where the person meets it.
- Full public global chat.
- Permanent global user directory.
- Multi-device person sync or global account identity beyond the signed Profile
  document and its current device bindings.
- Profile-level device pairing and cross-device sync UX before the Profile-DID
  contact model is complete.
- Contact blocking/moderation beyond simple Accept, Decline, and Remove.
- Rich moderation system.
- Separate `contacts`, `profile`, and `conversations` WASM capsules.
- Large refactor of `room_service.rs`.
- Exposing runtime/device DIDs in normal UI.

## Design Rules

- Prefer explicit user actions over ambient discovery.
- Keep permissions contextual: ask when the user invites, accepts, declines, or
  shares, not at app launch.
- Use progressive disclosure: normal UI shows people and conversations;
  advanced UI can show DIDs, roles, key epochs, and transport diagnostics.
- Make every cross-runtime action visible in Inbox or Chat history.
- Keep deterministic paths (QR/code/link) even after discovery exists.
- Optimize for "works between my server and Mac" before adding broad social
  behavior.

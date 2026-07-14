# People, Pairing, Friends, and Conversations

Status: current product/UX contract. The implemented path covers conversation
copy, signed peer invite links, a principal profile-card object, signed profile
propagation, `people.contacts[]`, and a standalone People app with opt-in peer
discovery. Full FriendRequest objects, Inbox acceptance, timed discoverability,
deny/block handling, and separate direct-conversation objects remain planned.

## Goal

Make it easy for ElastOS users and devices to find each other, approve trust, and
chat/share objects without exposing runtime DIDs, provider names, or transport
details in the main UI.

The first useful product outcome is simple:

- I can pair my Mac/server/Jetson as my own device.
- I can add another person as a friend/contact.
- I can open Chat and see conversations, not runtime plumbing.
- I can share a document/object into a conversation and open it in ElastOS.

## Core Decision

Do not create separate `friends`, `profile`, and `conversations` WASM capsules
yet. That is likely correct later, but it is too much surface area before the
object model and UX are proven.

For the near term:

- `Profile` is a System module backed by profile objects.
- `Friends` / `People` is a Home section backed by contact objects.
- `Conversations` is a Chat Room module/view backed by conversation objects.
- `Inbox` remains the approval surface for friend, pairing, and room requests.

Split these into separate capsules only after the user-visible flow is stable
and the provider/object contracts are small enough to review.

## Ontology

Use these concepts consistently:

- `ProfileCard`: the public card a person chooses to share. It contains display
  name, optional avatar/color, short handle, profile DID, and chosen public
  metadata. It must not leak raw runtime/device IDs.
- `DeviceBinding`: a trusted device/runtime bound to the same principal. This is
  for "Pair my device", not "Add friend".
- `DiscoveryAnnouncement`: an ephemeral signed profile card published while
  discoverability is enabled.
- `FriendRequest`: an Inbox object asking to exchange profile/contact trust.
- `Friend`: an accepted contact object under the user's principal root.
- `Conversation`: a direct, group, or public thread between profile identities.
- `Message`: a signed object in a conversation.
- `Attachment`: a byte-backed Chat attachment or object handle shared into a
  conversation.

Runtime/device DIDs are allowed as internal transport/signing material, but they
are not the product identity shown to users.

## UX Model

### System / Profile

User-facing actions:

- Edit my profile card.
- Pair my device.
- Turn discovery on for a limited time.
- Review paired devices.

Copy rules:

- Say "device", "profile", "people", "conversation".
- Do not say "runtime DID" in ordinary UI.
- If a DID is shown in advanced details, label it as technical proof material.

### People / Friends

User-facing actions:

- See accepted friends.
- See nearby/discoverable ElastOS users.
- Send friend request.
- Start chat.
- Block or remove contact.

Discovery must be opt-in and time-limited. The primary button should read
`Discoverable for 10 minutes`, with an obvious stop action.

### Inbox

Inbox owns approval:

- `Alice wants to add you as a friend`.
- `Pair MacBook as your device`.
- `Alice invited you to a conversation`.

Each request needs clear Accept/Deny actions and enough context to decide. Do
not ask for broad permission before the user initiates or receives a concrete
request.

### Chat / Conversations

Chat should show conversations:

- `Global` later, if/when public global chat is intentionally added.
- Direct conversations with friends.
- Group conversations.
- The current managed/shared room can remain as the first group-conversation
  implementation, but the UI should present participants, not owners/admins.

Near-term UI copy should avoid defaulting to "owner/admin/member" unless the
user opens advanced room controls. The normal participant list should show
profile names and devices, for example `Alex`, `MacA`, or `Alex on MacBook`.

## UI Patterns

### Profile Card

Use one compact card everywhere:

- avatar/color mark
- display name
- short handle or profile proof badge when available
- relationship state: `You`, `Paired device`, `Friend`, `Pending`, `Blocked`
- primary action: `Message`, `Add Friend`, `Accept`, or `Pair`

Do not show raw DID text on the card. Put technical identifiers behind
`Details`.

### Pair My Device Sheet

Use a focused sheet with three equivalent ways to pair:

- QR code
- short code
- copy link

Show the expiry plainly: `Expires in 10 minutes`. The receiver should see:
`Pair this Mac with Alex?` and a short explanation of what pairing grants.

### Discovery Panel

Discovery should feel like AirDrop/Bluetooth, but with stronger consent:

- big toggle: `Discoverable for 10 minutes`
- countdown pill while active
- visible `Stop` action
- discovered profile cards in a simple grid/list
- empty state: `No discoverable ElastOS users nearby yet`

The panel should never imply background permanent visibility.

### Inbox Request Card

Every request card should answer:

- who is asking
- what they want
- what accepting allows
- what data will be shared

Example:

`Alice wants to add you as a friend. Accepting shares your profile card and lets
Alice start a direct conversation with you.`

Actions: `Accept`, `Deny`, `Block`.

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

1. User turns on `Discoverable for 10 minutes`.
2. Runtime publishes a minimal signed `DiscoveryAnnouncement` to a public
   Carrier discovery topic.
3. Other discoverable users see the profile card.
4. User clicks `Add Friend`.
5. Recipient receives an Inbox `FriendRequest`.
6. Accept creates `Friend` objects on both sides and allows Chat to start a
   direct conversation.

Privacy constraints:

- Off by default.
- Time-limited.
- User can stop immediately.
- Profile card is deliberately minimal.
- Use ephemeral announcement ids; avoid stable tracking identifiers in the
  discovery layer where possible.
- Block is local and immediate.

## QR, Code, and Link Flow

QR/code/link remains the deterministic fallback and should ship first.

Use it for:

- Pair my device.
- Invite someone to a conversation.
- Join a shared room from another runtime.

The invite payload should contain:

- invitation kind: device pairing, friend request, or conversation invite
- inviter profile card summary
- room/conversation id when relevant
- Carrier bootstrap/rendezvous material
- short-lived signed secret
- expiry

It should not require the sender to type the receiver's DID before sharing the
invite. The receiver introduces its profile/device proof during accept.

## Implementation Slices

### Slice 1 - Copy and Mental Model Cleanup

Status: implemented.

Scope:

- Keep current Chat behavior.
- Change visible labels away from owner/runtime language where possible.
- Keep advanced room controls available but not central.
- Keep `scripts/chat-room-live-roster-smoke.sh` as the non-mutating roster
  proof.

User test:

- Open Chat on public Linux and Mac.
- Confirm participant labels make sense.
- Send text and a document attachment both ways.
- Confirm no runtime DID/owner/admin wording appears in the normal path.

### Slice 2 - Join Shared Conversation by Code

Status: implemented for signed peer invite links.

Scope:

- Add a simple invite-code/link flow over the existing managed-room backend.
- Receiver accepts without manually entering a DID.
- Accepted participant appears by profile/device label.

User test:

- Server creates invite code.
- Mac joins using code.
- Both sides see each other and exchange messages/attachments.

### Slice 3 - Profile Card Object

Status: implemented.

Scope:

- Add a small profile card object under the principal root.
- Surface display name/device label consistently in Home/Chat.
- Keep editing in People.

User test:

- Change profile display name.
- Chat and People reflect the new label without exposing DID.

### Slice 4 - People/Friends Read Model

Status: partially implemented.

Scope:

- Add a minimal People/Friends view backed by accepted contact objects.
- Start chat from a friend card.
- Keep Inbox as the approval surface.

User test:

- Accept friend request.
- Friend appears in People.
- `Message` opens or creates a direct conversation.

Current implementation note:

- Accepted Chat conversation members are the first contact source.
- Signed join/accept envelopes carry optional profile-card summaries.
- Home summary exposes `people.contacts[]`.
- The People capsule renders contacts and opens Chat from `Chat` through a
  validated Home intent.
- The People capsule also renders opt-in peer discovery backed by Carrier gossip:
  visible peers can be requested, receivers can accept, requesters can join, and
  the resulting signed invite/acceptance creates the first People contact.
- Dedicated `FriendRequest` objects, Inbox approval, timed discoverability, and
  deny/block handling are not implemented yet.

### Slice 5 - Opt-In Discovery

Status: partially implemented.

Scope:

- Publish/read ephemeral Carrier discovery announcements.
- Send a People request to a discovered peer.
- Accept the request from People.
- Let the requester join through a DID-targeted signed peer invite.
- Keep public invite URIs canonical as `elastos://peer/invite`.

Remaining:

- Add `Discoverable for 10 minutes` with countdown and auto-off.
- Move request acceptance into Inbox-backed `FriendRequest` objects.
- Add deny/block handling.
- Create direct-conversation objects instead of relying only on the shared Chat
  conversation contact source.

User test:

- Turn discovery on from one runtime.
- See it from another runtime.
- Send request, accept in People, join, then chat.

## Near-Term Non-Goals

- Full public global chat.
- Permanent global user directory.
- Rich moderation system.
- Separate `friends`, `profile`, and `conversations` WASM capsules.
- Large refactor of `room_service.rs`.
- Exposing runtime/device DIDs in normal UI.

## Design Rules

- Prefer explicit user actions over ambient discovery.
- Keep permissions contextual: ask when the user pairs, invites, accepts, or
  shares, not at app launch.
- Use progressive disclosure: normal UI shows people and conversations;
  advanced UI can show DIDs, roles, key epochs, and transport diagnostics.
- Make every cross-runtime action visible in Inbox or Chat history.
- Keep deterministic paths (QR/code/link) even after discovery exists.
- Optimize for "works between my server and Mac" before adding broad social
  behavior.

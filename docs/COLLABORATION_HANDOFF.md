# Collaboration handoff

This document defines the People and Chat boundary and its acceptance sequence.
Current refs, source evidence and release scope live in [GitHub issues](https://github.com/Elacity/elastos-runtime/issues).

## Evidence boundary

Installed acceptance requires matching receipts for the reviewed candidate.
HTTP 200 and source fixtures do not establish full product acceptance. Record
each source or installed result in its GitHub issue.

Use `git rev-parse HEAD HEAD^{tree}` and `git status --short --branch` for the
exact reviewed commit, tree, and worktree status. Do not copy an old commit ID
from this document after a local history reconstruction.

## Product model

1. A person is a signed Profile DID.
2. A Profile authorizes endpoint DIDs for delivery and signer DIDs for scoped
   application actions. An endpoint or signer is not a person.
3. Runtime derives identity and authority. It owns protected collaboration
   state, workers, routing, lifecycle, and audit.
4. Carrier authenticates and transports Runtime endpoints. It does not decide
   who a person is, who is a contact, or who may read a message.
5. Capsules call typed Runtime routes. People, Chat, and Inbox do not select
   Carrier peers, routes, tickets, ports, or provider topology.
6. Discovery is explicit, temporary, bounded, and opt-in. It is not a global
   user list.
7. Inbox is the only Accept or Decline authority surface.
8. A direct conversation is authorized between accepted Profile identities.
   Its stable conversation ID is an opaque selector, not authority.
9. Shared-room attribution comes from verified signed Profiles. Presence is
   liveness only.
10. A delivery receipt proves that the named Runtime endpoint accepted the
    exact envelope. It does not claim that a person read the message.
11. The signed network profile and seed signer provide bootstrap and
    configuration authority only.
12. Each collaboration object has one canonical signed schema. Earlier drafts
    require an explicit migration decision rather than compatibility decoding.

## Implemented source boundary

### Configuration and transport

- An owner-only signed collaboration startup profile selects the network,
  trusted profile signer, bootstrap peers, and optional default conversation.
- Missing configuration selects isolation. Invalid configuration fails closed.
- Runtime owns one long-lived Carrier endpoint and verifies the requested peer
  identity before provider dispatch.
- Collaboration workers start and stop with Runtime. No capsule starts a
  transport worker.

### Profile and contacts

- Profile creation and update require a proof-bound signed-in session.
- Reads do not create a Profile, device key, or collaboration state.
- Contact requests bind to signed Profile advertisements.
- Contact decisions are signed and reviewed through Inbox.
- Contact state is keyed by Profile DID and includes requested, declined,
  connected, removed, and removed-you states.
- Profile updates use a bounded signed revision chain. Replays are idempotent;
  rollback, conflict, gaps, wrong Profile, and unauthorized signers fail.
- Bilateral removal uses a signed pair-scoped revocation. Both sides retain an
  honest visible state and readable history under Chat's declared policy.
- Recovery carries the Profile authority and signed contact store. A restored
  Profile keeps its Profile DID and authorizes the new endpoint through the
  normal signed update path.

### Chat

- Chat opens direct conversations with an opaque conversation ID from People.
- Runtime enforces the current contact relationship and Chat's declared
  history policy on every direct read and send.
- Direct messages use the accepted Profile's current authorized endpoint.
- Runtime stores the signed envelope before its first delivery attempt and
  retries within the declared lifetime.
- Shared-room messages and participant rows use verified Profile names.
- Runtime retains canonical signed Community message originals for up to 24 hours,
  with at most 200 messages and a 1 MiB limit on the complete serialized history
  sidecar. The sidecar uses the existing conversation namespace and owner-only
  state lock. The live state format remains readable after Undo.
- Community catch-up reads those originals from online participants through the
  private `collaboration.history` Runtime service. A request carries live signed
  presence; Runtime verifies its Profile, conversation grant, and authenticated
  Carrier endpoint. Every returned original passes signature, Profile, exact
  conversation, Chat payload, and historical age checks before projection.
- Catch-up runs in its own bounded Runtime task. Live delivery keeps its five-minute
  lifetime. An unavailable peer history produces an explicit catch-up status while
  the local conversation remains available.
- Stale asynchronous Chat results cannot replace the selected conversation.
- Direct messages are text-only in this source boundary. The UI states that
  attachments are unavailable instead of hiding or inventing a path.
- Chat receives only product status from Runtime. Gossip topics and peer counts
  are not part of its read model.

### User interface

- People shows Profile identity, discovery, request state, contacts, removal,
  and reachability without device or route labels.
- Chat supports shared and direct selection without stale-selection races.
- Inbox remains the decision surface.
- Home provides the trusted Clipboard path.
- Shared UI assets and the selected Home shell work are included as separate
  review slices from the collaboration authority work.

## Source acceptance coverage

The fixture-owned two-Runtime journey must use fresh disposable data roots and
cover:

- Recovery and Profile creation;
- overlapping opt-in discovery;
- one contact request and Inbox acceptance;
- direct messages in both directions;
- Profile rename propagation;
- bilateral removal and re-add;
- shared-room continuity;
- restart of both Runtimes;
- direct and shared history after restart;
- trusted Clipboard use;
- narrow-window People and Chat checks;
- final Profile-name and identity scans.

Current source and platform CI checkpoints are recorded in [GitHub issues](https://github.com/Elacity/elastos-runtime/issues).
Each final candidate requires its own checks and installed acceptance.

The Carrier dependency-generation check verifies a coordinated transport
dependency graph. Source and fixture tests remain separate from the installed
two-Runtime acceptance below.

## Next acceptance steps

1. Complete final candidate review and CI, preserving the reviewed history.
2. Complete one-Runtime Profile, People, Chat, Inbox, Clipboard, restart, and layout
   behavior with the existing local data preserved.
3. Install the same exact candidate on the approved isolated acceptance Homes.
   Keep the install/release seed in its assigned role.
4. Run the complete journey with three separate web accounts and two self-hosted
   Homes, including a Home behind a router. Use the approved Community bootstrap
   and relay placement recorded in the owning issue.
5. Record each target's source/artifact identity and product verdict before
   making release or installed-acceptance claims.

## Later work

The following work is not part of this candidate:

- encrypted mailbox delivery for a sender or recipient that remains offline;
- general group catch-up beyond J6's bounded public-room history;
- user-created group identity and signed membership;
- silent block as a separate local action from removal;
- broad discovery rendezvous and abuse controls;
- multi-device pairing UX;
- direct-message attachments.

These items remain in [deferred work](DEFERRED_WORK.md#collaboration-identity-and-carrier-boundary). Do not add a fallback transport or widen the
current candidate to implement them during acceptance.

## Verification commands

```bash
git diff --check
node scripts/check-product-data.mjs
cargo test --manifest-path capsules/chat-room-ui/Cargo.toml
cargo clippy --manifest-path capsules/chat-room-ui/Cargo.toml --all-targets -- -D warnings
(cd elastos && cargo fmt --all -- --check)
cargo fmt --manifest-path capsules/chain-provider/Cargo.toml -- --check
node --test scripts/home-two-runtime-acceptance.test.mjs
just verify
```

For the complete fixture-owned product proof, use
`scripts/home-two-runtime-acceptance.mjs` with fresh fixture configuration. Do
not run it against normal localhost or the public seed unless the task
explicitly authorizes those installations and data roots.

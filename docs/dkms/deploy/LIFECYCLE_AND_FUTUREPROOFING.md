# Lock-in, Rotation & Future-Proofing — the "before we commit" briefing

Your core worry, answered plainly: **nothing we do at 2-of-3 today locks us in or strands assets
later.** The system was designed for membership and threshold to *change over time* — that's not a
future bolt-on, it's first-class in the node protocol. This doc shows the evidence, maps it to your
DAO-council vision, reconciles the transport with the runtime principles, and lays out the ela.city
playback path.

---

## 1. Will assets minted today still decrypt after we add / rotate nodes? — YES

Three node operations make this safe, and they're **already in the node protocol** (the daemon's
`supported_operations`):

| Operation | What it does | Effect on old assets |
|---|---|---|
| `RotateShare` | re-escrow ONE node's share to a successor node (refreshed), operator-signed | swap a single node out; CEK unchanged, asset still opens |
| `ReshareContribute` / `ReshareInstall` | re-share a live `t`-of-`n` set into a **new `k`-of-`m`** set | change membership AND threshold; CEK unchanged |
| `RevokeCaller` | operator-signed live revocation of a caller | access control only; keys untouched |

**Why old assets keep working — the math, not a promise.** Reconfiguration re-shares the *existing*
shares onto a fresh polynomial whose constant term is the **same CEK**: `P(0) = Σ λ_i·q_i(0) =
Σ λ_i·p(x_i) = p(0) = CEK`. The key is never reassembled during the move, yet the new set
reconstructs the identical CEK. This is **proven by a passing test** that re-shares a live
**2-of-3 into a 3-of-5** and confirms any 3 new shares rebuild the original key
(`ddrm-envelope::reshare_2of3_to_3of5_keeps_cek_and_lifts_the_threshold`).

So: **mint today to 2-of-3 → later reshare to (say) 5-of-12 → every asset minted today still opens**,
with no re-encryption of content and no re-minting on-chain. The on-chain asset never changes; only
the off-chain share layout does.

**Bonus security property — old shares die.** After a reshare, OLD share material is *garbage*
against the new set (the same test asserts it). So a node that leaves the council — or one that was
compromised before rotation — holds nothing useful afterward. This is exactly the forward-security
you want for yearly-rotating governance nodes.

### What's shipped vs. what to promote (honest status)

- **Shipped + verified:** the node ops, the reshare/DKG crypto, the AAD binding + operator
  authorization + node-set re-pinning, and the end-to-end drivers in the runtime orchestrator
  (the dry-run exercises them).
- **To promote (roadmap, low-risk):** a standalone **operator-console coordinator command** that
  drives a reshare/rotation across your *live deployed* nodes and republishes the descriptor — same
  category and effort as the DKG ceremony coordinator (the hard parts are done). Until then,
  rotation is driven through the orchestrator path rather than a one-line operator command.

**Bottom line: 2-of-3 today is a starting point, not a ceiling. No lock-in.**

---

## 2. Mapping to the DAO-council vision (ELA-staked governance nodes)

Your model — governance nodes elected yearly via ELA voting, operators staking ELA as slashable
collateral — layers **economic security** on top of the **cryptographic threshold**. They compose
cleanly:

- **Governance nodes ⇄ dKMS nodes.** A council node runs a `dkms-authority`; its stake is the
  collateral, slashable for misbehavior (e.g. refusing valid recovers, or attempting collusion that
  attestation exposes).
- **Yearly rotation ⇄ an authorized reshare.** When the council turns over, the operator authorizes
  a reshare from the outgoing set to the incoming set. Content is untouched; outgoing nodes' shares
  die; the node-set id is re-pinned to the new council.
- **The threshold is your collusion bound.** With `k`-of-`n`, you need `k` staked council nodes to
  collude to recover a key — and `n−k+1` honest nodes to keep service alive. Pick `k`/`n` to match
  the council size and your trust assumptions (e.g. a 12-seat council might run 5-of-9 or 7-of-12).

**The ONE rule to protect old assets across a rotation:** a handover must be an **authorized
reshare** (shares move to the new set preserving the CEK), *never* a fresh DKG + store wipe (which
would abandon the shares old content is bound to). The kit's rollback guidance already says "never
delete a node's master.seed" for the same reason.

**Defer the full decentralization to the roadmap** (as you said). Today's deployment can run on your
company nodes (Interserver/Contabo/third); migrating those seats to elected DAO nodes later is itself
just a reshare. So starting company-operated does **not** block the DAO end-state.

---

## 3. Transport & the runtime principles — reassurance

**First, a correction:** the dKMS rail is **not HTTP**. It's a **length-prefixed framed JSON
request/response** over a raw TCP (or Unix) socket, wrapped in an app-layer **post-quantum
mutually-authenticated, encrypted, replay-bound channel**. (HTTP shows up elsewhere — the browser
gateway — but never on the key-authority wire. PC2's key wire *is* HTTPS with verification off; ours
is the opposite.)

**Does a dedicated socket violate Principle #4 ("Carrier plane for local and off-box")? No — it
honours the principles, here's why:**

- **#2 Stable Identity Over Transport.** The node is named by its **stable cryptographic identity**
  (verifying key + escrow recipient), pinned in the descriptor. The `tcp:HOST:PORT` is a *delivery
  adapter*, not the node's identity — swap the endpoint, same node. This is principle #2 verbatim.
- **#4 says the capsule contract sits ABOVE the transport.** Inside the runtime, the capsule speaks
  a **key-release provider contract** (`KeyReleaseRequestV1`) — a Carrier-shaped signed request with
  a target, action, response, and audit. Principle #4 explicitly lists "loopback, HTTP, WebSocket,
  stdio, in-process" as **host adapters below the capsule contract** — the framed dKMS socket is one
  more such adapter. The *capsule* never speaks raw sockets; `key-provider` (a provider) does, below
  the contract.
- **#3 No Ambient Authority.** The channel is the opposite of ambient: callers are explicitly
  allow-listed, every recover needs a live node-verified session token, lifecycle ops need operator
  signatures, and `RevokeCaller` makes authority revocable. Missing/forged authority fails closed.
- **#5 Small Trusted Core.** The key authority is deliberately a small, separate, auditable service
  with a minimal wire — not coupled to the whole p2p/gossip stack.

**Why a dedicated channel is the *better* choice (not just acceptable):** the key authority needs a
**point-to-point, identity-pinned, attested, low-latency** link to an *external trust domain* — not
gossip/DHT discovery. Putting it on the Carrier mesh would enlarge its attack surface and couple a
crown-jewel service to the entire p2p layer. Industry (and PC2-via-Lit) treats key authorities the
same way: you *call* them over an authenticated channel; you don't gossip with them.

**Future alignment:** the CARRIER.md end-state ("one capability plane") could front this with a
Carrier capability *adapter* without changing the underlying authenticated transport — so this is
forward-compatible with the principles, not a deviation to unwind later.

---

## 4. ela.city playback of dKMS-minted assets — your intuition is right

You framed it perfectly: *"instead of taking the Lit key it takes the dKMS key."* That's exactly the
shape, because **rights are already shared** (same Base contracts) — only the **key-delivery call**
differs. A dKMS-minted asset is a normal on-chain asset, so it **shows up in the ela.city
marketplace** like any other (listing is authority-agnostic). The only authority-specific step is
playback. Two viable shapes (not mutually exclusive):

1. **ela.city player as a runtime capsule** (the "elacity in elastOS" end-state). The player runs as
   a capsule served by the runtime gateway and uses the native decrypt rail (dKMS) directly. Cleanest
   long-term; this is the fully-converged 10/10.
2. **ela.city stays a hosted web app, its player branches on authority.** For native assets the
   player calls a **browser-reachable dKMS recovery endpoint** (a runtime gateway that proxies to the
   quorum) instead of Lit's `recoverCEKEnvelope`; for legacy assets it keeps calling Lit. This is the
   "new assets play on ela.city, old Lit assets unchanged" outcome you described.

**Reassurance on lock-in here too:** minting to dKMS today does **not** strand an asset from ela.city
playback. The asset is on-chain (discoverable) and its CEK can be delivered to whichever player you
build, because **you control the quorum and the gateway**. The work to make the ela.city player take
the dKMS key is real and on the roadmap — but it is *never blocked* by today's choices. The only
anti-pattern to avoid is publishing native content with **no** browser-reachable recovery path; since
you own the quorum, that path is always available to open.

---

## TL;DR

- Old assets survive **adding nodes**, **rotating nodes**, and **raising the threshold** — proven by
  the CEK-invariant reshare (2-of-3 → 3-of-5 tested), and the node supports it natively.
- Yearly DAO-council rotation = an **authorized reshare** (never a wipe); staked-ELA economic
  security composes on top of the cryptographic threshold.
- The transport is a hardened framed PQ channel (**not HTTP**) and is **principle-aligned** —
  identity-over-transport, contract-above-adapter, no ambient authority.
- ela.city **will** show + play dKMS-minted assets (player takes the dKMS key); nothing today blocks
  it. Legacy Lit content stays on Lit.
- **No lock-in. Start at 2-of-3 with confidence.**

# THE ELACITY BIBLE

> Narrative and brand canon for Elacity, ElastOS, and Elacity Labs. This is a
> story and messaging document, not a runtime behavior contract. For current
> proven behavior, see [state.md](../../state.md).

*In this book, shipped claims are real — you can check the code — and direction is marked as direction. Our engineering law says: "no pretending a feature is supported when it is only half-implemented." The same law binds these words.*

---

## I. The Creed

*(To be read aloud. Ninety seconds.)*

We owned our first computers.

The machine on the desk answered to the person in the chair. Your files were yours because they sat on your disk. Your programs ran on your silicon. Ownership was not a license clause. It was a fact you could touch.

Then, one convenience at a time, we were moved out. Our documents went into someone else's building. Our names became rows in someone else's database. Our work fed platforms that keep half and can erase us overnight, without appeal. The cloud called it your account.

It was always their house.

Now the machines have begun to act. AI agents hold our keys, our money, our words — and they run as tenants in other people's clouds, on credentials that leak by the million. The industry has admitted the hard part out loud: an agent can always be fooled. So the only safety left is an old idea, older than the internet: limit what a fooled servant can touch.

We are building the house where that is law.

A computer that answers to you. Where nothing — no app, no browser, no AI — acts without a key you granted. Where every key opens one door, for one purpose, for a limited time, and every turn of every key leaves a receipt. Where your work travels with its own lock, its own rights, and its own till. Where humans and AI live under one law, and the machine's rules are stricter, never looser. Where even the landlord cannot enter the guest's room.

We publish what is not done yet. We fail closed, then explain. We would rather stop than pretend.

The internet made you a tenant.

This is the deed.

---

## II. The Story

*(This is the heart of the book. It is written to be read, not skimmed.)*

### The question

Every era of computing has been an answer to a single question: who does the machine work for?

For the first thirty years the answer was plain and disappointing. The mainframe was a temple. You did not own a computer; you petitioned one. Your job deck went in through a priesthood of operators, and the machine's loyalty ran upward, to the institution that paid for the raised floor. Computing was something done *to* people — at best, on their behalf.

Then came the heresy. In 1977 the Apple II shipped, and the phrase *personal computer* stopped being a contradiction. The claim was not about transistor counts. It was moral. Alan Kay's Dynabook, Engelbart's augmentation research, the whole Xerox PARC ferment — these were arguments that a computer should amplify one person, and be owned the way a book or a bicycle is owned: an instrument whose loyalty runs to the hand that holds it. For about fifteen years, the industry believed it. The files were yours because they sat on your disk. Authority and ownership lived together in a beige box on a desk.

Kay's later verdict — the computer revolution hasn't happened yet — still stands. We shipped the hardware of personal computing without its deeper idea. And he delivered it, with hindsight's cruelty, at the very moment the counter-revolution was gathering.

### The exile

The web, which promised to connect personal computers, quietly inverted them.

The browser began as a window and became a landlord's office. Software moved off your disk and into "the cloud" — which is to say, onto someone else's mainframe. The pendulum swung back to the temple, this time with better fonts. By the 2010s the settlement was complete, and it deserves its plain name: platform feudalism. You farm your own attention on land you do not own. The platform keeps 45 percent of the harvest — that number is YouTube's own published split; other landlords post 30, or steeper. An algorithm change can cut your reach in half overnight, with no warning and no appeal. An account ban is exile without trial.

The purest architecture of this settlement was ChromeOS — a "computer" that is, by design, nothing but a browser. A terminal to somebody else's machine.

Understand the trade that got us here, because it was not stupid. The old personal computer had a fatal flaw: every program you ran inherited *all* of your power. Any app could read any file, touch the whole network, act with your entire authority. That is why the PC era drowned in viruses. The cloud fixed the fragility — by confiscating the sovereignty. You got safety as a subscription, in someone else's house, under someone else's law.

Nobody finished the third option: a machine you own that does not trust its own software. Hold that thought. It is the whole book.

### The man who remembered

Every exile story carries someone who remembers the homeland. Here, he is literal.

Rong Chen arrived in New York on January 4, 1984 — the ARPANET era, the year of the Macintosh — and spent some seven years studying operating systems at the University of Illinois Urbana-Champaign. In 1987 he interned at NCSA, the lab that would soon birth the Mosaic browser, writing code to pull data off Cray supercomputers and draw it on SUN workstations. In 1992 he joined Microsoft, where his credited work runs through the heart of component software — OLE Automation, DCOM, ActiveX. In 1995 he became — by his own count — the tenth member of the Internet Explorer team.

He was standing at the exact hinge where the browser began to swallow the operating system.

And in April 2000 he resigned over it — over Microsoft's decision, as he tells it, on the future of the COM component model — and went back to China to build the road not taken: an operating system designed *for the network* rather than for the box. His doctrine, stated in nearly the same words for a quarter century: third-party apps must be sandboxed so they cannot abuse the first-party user's data, and the operating system, as the second party, must provide that secure environment. Apps never touch the internet directly. The network hides beneath them like a computer's internal bus.

Read that doctrine again. It is a description of ElastOS, written before ElastOS had a name.

### Too early, twice

Through the 2000s, in Beijing and then Shanghai, Chen's team built an operating system from scratch — boot loader, kernel, graphics, network stack — around CAR, a C++ component runtime descended directly from his COM work. Ecosystem histories say an Elastos smartphone reached the edge of mass production by 2007; treat that as an attempted commercialization, not a market victory. Foxconn invested roughly 200 million RMB in 2013 for industrial and smart-home work. The system was real. The moment was wrong. An operating system for the network needs something no single company can manufacture: a trust layer that belongs to no one.

In 2017, Chen found it. Blockchain was the missing piece — not as a casino, but as the neutral root of trust his architecture had always lacked. The Elastos Foundation launched that year; a large ICO followed in January 2018; and then, unusually for that era, the project actually built things:

- A mainchain **merge-mined with Bitcoin** since August 26, 2018. Bitcoin's own miners secure the Elastos chain as a byproduct of work they already do — an arrangement Satoshi himself sketched in December 2010. The chain's native asset, ELA, is a working part, not a mascot: it settles the Elastos chains and denominates the DAO treasury that funds the work in this book. Its function gets accounted for here; its price never does.
- A **DID sidechain** implementing the W3C decentralized-identity standard: identity as something you hold, not something you are issued.
- **Carrier**, a serverless peer-to-peer network, identity-addressed and encrypted, with no raw IP addresses at its surface. By January 2019 it counted over a million nodes — though honesty keeps the fine print: those nodes were TV-box deployments through a single hardware partner. Distribution, not adoption.

And then, the stall. The 2018 crash gutted the token. The consumer surface was abandoned in a 2021 pivot toward a wallet. Elastos entered the 2020s as the strangest artifact in the industry: Bitcoin-grade security, standards-grade identity, planetary-scale transport — and no product.

World-class plumbing for a house nobody had built.

### The second wound

Every return story needs a second protagonist with a fresher wound.

Sasha Mitchell ran 3-D capture work — for Disney, Warner Bros., Universal, and Netflix, as he tells it — digitizing actors' faces, bodies, and performances with photogrammetry and LiDAR. In those capture rooms, the exile story compressed to a single human face. One famous actor described being scanned as having his soul taken. Others asked for a copy of their own avatar — and were told the studio owned it.

Your likeness. Your work. Your data. Held in a house that is not yours.

Everything Mitchell has built since is one long answer to that room. In 2021 he founded Elacity inside the Elastos community — a marketplace on the Elastos Smart Chain. In September 2022 he took a rights-management proposal to the ecosystem's own DAO, asking that funds be released only against delivered work. The elected council passed it unanimously, twelve to zero — the vote sits in the DAO's public record. By January 2024 Elacity's decentralized rights system was live and commercial, and the marketplace could do something genuinely new. A creator uploads a work. The work is encrypted into a **Digital Capsule**. And what is sold is not a copy but a key — access, royalties split to a tenth of a percent and paid in the instant of sale, resale terms written into the asset itself. Elacity's published fee is 2 percent. The platforms it replaces publish cuts of 30 to 55.

The NFT era had just died of a specific disease — roughly 96 percent of collections dead, by one widely reported study — and its autopsy reads in three words: receipts without locks. Tokens that asserted ownership of content nothing enforced. A Capsule is the answer to that autopsy: the work travels sealed, with its rights and its payment built in.

### The joining

By early 2025 the two arcs — the exiled architect and the wounded craftsman — converged into one plan. On January 31, 2025 the Elastos community approved the World Computer Initiative — proposal and vote on-chain, like every mandate in this story: turn twenty-five years of hidden infrastructure into a computer a person can actually hold. Elacity Labs — Mitchell as CEO, Anders Alm as CTO — forked the open-source internet OS Puter into **PC2**, the Personal Cloud Computer: your files, identity, wallets, and AI on hardware you control, reachable from anywhere. And beneath it they began the deeper work: a from-scratch, capability-secured core — the **ElastOS Runtime** — written in Rust, built to be small enough to reason about and strict enough to deserve trust.

On February 10, 2026, Elastos World Computer V1 launched, and Rong Chen put three million dollars of his own conviction behind it — the "Keystone Gift," announced with the launch, held in DAO custody, released tranche by tranche only by public on-chain vote. The official launch announcement carried the most honest sentence in the project's history:

> "For the first time in eight years, Elastos has a working core product."

The dream never changed; the missing pieces arrived in stages. The 1980s gave the scholarship. The 1990s gave component software. The 2000s gave a full OS, built too early. 2017 gave the trust layer. 2024 gave the rights engine. And 2026 gave what none of the pieces could be alone: a shipping product line, a live marketplace, and a hardening sovereign core — funded not by a venture round but by the ecosystem's own treasury, in public, vote by vote.

### The agent age

Then the world changed shape again, and a twenty-five-year-old doctrine stopped being philosophy and became emergency response.

In 2025 and 2026, AI agents arrived at consumer scale — and their first mass deployment was an uncontrolled experiment in ambient authority. OpenClaw, a viral open-source personal agent, gathered 135,000 GitHub stars in weeks — the counter is GitHub's own. Within days, published security scans counted roughly twenty-one thousand instances exposed on the open internet, leaking the API keys and OAuth tokens their owners had handed them, while hundreds of malicious skills seeded its marketplace. Cisco's headline said it plainly: personal AI agents like this are a security nightmare. The public CVE registry logged more than thirty entries against the agent-tool protocol MCP in the first two months of 2026 alone. Industry surveys put agent over-permissioning near nine in ten. Secret-scanning firms counted credentials leaked into public code by the tens of millions in a single year.

And then came the concession that reframes everything. OpenAI's own security chief called prompt injection a frontier, unsolved problem — one unlikely ever to be fully solved. The admission is public and on the record.

If an agent's *inputs* can always be poisoned — if some sufficiently clever string of text can turn your assistant into an adversary's — then the input side of agent safety is lost, by the industry's own admission, permanently. The only durable defense is on the *output* side: bounding what a fooled agent can do. Scoped. Expiring. Revocable. Audited. Fail-closed.

There is a name for that discipline. It has run like recessive DNA through computer science since 1966 — capability security — and it is the exact thing Rong Chen bet his career on in 2000, and the exact thing Elacity Labs has been compiling into Rust ever since. Even Microsoft now agrees with the diagnosis: at Build 2026 it reframed Windows around per-agent identity and containment. But look where Microsoft roots the authority — in its own cloud tenant, with your agents' workspaces offered for rent. The feudal answer to the agent age is to make your AI a tenant of their house.

Your AI works for whoever holds its credentials.

So hold them.

### Where the story stands

The homecoming is not finished, and this book will not pretend otherwise. The runtime is pre-release; its own ledger calls version 0.5.0 a review candidate, not a release. The browser surface has not yet passed its own product proofs, and the team says so in public. The sovereign rights pipeline ships deliberately refusing to run until its backends are real. The house is framed, wired, and inspected — and the builders have nailed their unfinished-work list to the front door.

But the direction is no longer slideware. It is code that fails closed rather than promising open. The exile lasted a generation. The door, at last, is being hung on its hinges — and it is yours.

---

## III. What We Are Building

### One architecture, three surfaces

We build one thing that shows three faces.

**ElastOS** is the house: a sovereign operating layer for a person's whole digital life. Two codebases carry that name today, and we keep them honest. The shipped ElastOS product line — launched as Elastos World Computer V1 in February 2026, carrying desktop, personal storage, private AI, and wallets — runs on the earlier PC2 lineage. Beneath it, built from scratch in Rust, is the **ElastOS Runtime**: the capability-secured core this book describes, pre-release today. The rule of the handover is written down: the new core inherits PC2's protocol boundaries and its acceptance tests, never its monoliths. A hosted runtime lives at elastos.elacitylabs.com for anyone who wants to touch it.

**Elacity** is the market: ela.city, a live marketplace where creators encrypt work into Digital Capsules and sell keys instead of copies. Walk the shop as it runs today. A creator uploads a work and seals it into a Capsule. She lists the keys — per Elacity's published mechanics, three instruments deep: access tokens, minted from one to billions; royalty tokens, where a thousand tokens is one hundred percent of a work's revenue, splittable to a tenth of a percent; distribution tokens, writing resale terms into the asset itself. A fan buys in an ordinary checkout, and the work streams in Elacity's player, unlocked by the fan's key. The royalty split pays every named hand in the instant of the sale. Channels and subscriptions bundle ongoing access. The published fee is 2 percent. All of it runs today on Elacity's existing rights stack, while the runtime's stricter fail-closed pipeline is built to receive it.

**Elacity Labs** is the workshop: the company, led by Sasha Mitchell and Anders Alm, that builds both. It is a separate company from the legacy Foundation, and its money arrives in public: on-chain DAO mandates, every proposal published, every vote recorded, funds released tranche by tranche against delivered work — the same custody discipline that holds the Keystone Gift. Note what that structure does to truth-telling. A workshop paid only for proven work cannot afford overclaims, because an unproven claim is an unpaid tranche. The honesty discipline running through this book is not a virtue we advertise; it is the funding model we live under.

One sentence holds them together: the market gives the house a living economy, and the house gives the market a floor that cannot be seized.

And one note on chains, stated once so every later mention can be exact. Elacity's marketplace grew up on the Elastos Smart Chain — a sidechain with its own consensus, anchored to the Elastos mainchain that Bitcoin's miners merge-mine. The runtime's one proven live purchase path today runs against contracts on Base; the repository's own smoke tests pin that chain id, and the roadmap lists Base, ESC, and EID side by side as proof adapters. The direction, marked as direction: chains are providers behind one interface. No chain is the login. No chain is the lock.

### The four quadrants

The system balances across four planes. Each is defined as much by what it must never become as by what it does.

- **Home** — the human front door: your desktop, your Library, your people, your apps. It must never become policy logic or protocol plumbing.
- **Runtime** — isolation, verification, identity, keys, audit. It must never become app business logic, a social bridge, a wallet app, or a storage backend.
- **Carrier** — the roads between houses: an authenticated peer-to-peer plane for messages, objects, and streams. It must never become raw gossip exposed to apps, and it never replaces keys. It is the one piece that cannot be an app, for a bootstrapping reason with teeth: a capsule can't provide the transport needed to download itself.
- **Blockchain** — the land registry: identity anchors, provenance, publisher identity, settlement — anchored by the Bitcoin-merge-mined Elastos mainchain, with other chains attached as adapters. It must never become the app database or a mandatory gate on ordinary use. It is deliberately last in the build order: wallets and DIDs are proofs attached to a person's local authority — never the login itself.

Every effect in the system, local or remote, compresses to one line:

**capsule → runtime capability → provider plane → object or service.**

Whether the target is a file on your disk or a peer across the planet, the sentence is the same. That single sentence is the operating system.

### The house rules, in plain words

A few terms of art, each defined once, cleanly.

A **principal** is an owner of authority — a person, or an agent a person created.

A **proof binding** is a way of proving you are that principal: a passkey, a wallet signature, a decentralized identifier. The doctrine, verbatim from the wallet's own documentation: "A wallet address is a proof binding on a Runtime principal, not the principal itself." You are not your wallet. Your wallet is one of your keys to being you.

A **capsule** is sealed software — or sealed content. Signed, sandboxed, and born with nothing: no network, no files, no powers it did not ask for in writing.

A **capability** is the key. Not a badge that gets you past every guard, but a key cut for one lock: one resource, one action, a limited time, a counted number of uses, revocable at any moment. Lending is bounded like a physical key — a delegated capability can only open *fewer* doors than its parent, and can never be lent onward.

A **provider** is a licensed specialist: wallet, chain, content, rights, key-release, decrypt, network exit. Ordinary apps are forbidden by manifest law from even requesting raw power; providers earn their exception by declaring, in their signed manifest, why they hold it, exactly which operations they expose, and which audit events they emit. Authority here is declared, scoped, and inspectable — per capsule, in writing.

**Objects come before apps.** A photo is not `~/Photos/IMG_001.jpg`. It is a thing you own, with identity, provenance, and access control. Apps don't own content; they view it. Users open objects; the runtime picks the viewer. (Home as a full object browser is direction, marked as such — today Home launches apps, and the object model is being built underneath it.)

**Two namespaces carry the world.** `localhost://` is your local sovereign machine world — your rooms. `elastos://` is the shared world: identities, peers, and content addressed by what it *is*, not where it sits. The content is the identity — not the address. A gateway URL is convenience transport, never truth; content fetched from anywhere is verified against its own hash and signature before it is allowed to exist for you.

And the inversion that names the whole era: **the browser is a capsule, not the platform.** ChromeOS put the computer inside the browser. ElastOS puts the browser inside a computer you own — one sandboxed viewer among many, dangerous and treated as such, with no ambient off-box network of its own.

### The technical truth layer

For the reader who trusts nothing but code, here is what is actually built, in the repository, today.

The capability token is real cryptography: an Ed25519-signed structure binding a specific capsule, an issuer key, a resource pattern, an action, and constraints — expiry, revocation epoch, per-token use limits, delegation flag. Its byte layout is length-prefixed field by field so no two distinct tokens can collide under signature, with regression tests proving it. Every use passes **twelve validation checks in strict sequence** — and every failure, at every check, emits an audit event. Use counting is atomic: a concurrency test fires twenty simultaneous validations at a use-limited token and asserts that exactly the permitted number succeed. Enforcement is wired at every bridge a capsule can reach — the microVM channel, the HTTP handlers, the shell protocol — not decorated onto one path.

Isolation is not one sandbox but three, behind one contract: WebAssembly (fuel-metered, memory-capped, every guest pointer bounds-checked), Linux microVMs on KVM — where, if hardware virtualization is absent, the launch *fails* rather than degrading to something weaker — and macOS microVMs through a ten-thousand-line hand-written binding to Apple's virtualization framework. Same token, same wire protocol, same guest-visible world — with macOS proven today for browser-VM workloads, not yet at full parity as a general capsule substrate. The security model is built to outlive any particular sandbox technology.

The agent is a first-class citizen under the same law. The repo's agent capsule holds its own persona DID — cryptographically distinct from its human owner's, with the owner recorded; a test literally asserts "Persona DID must differ from owner." It signs every message it sends. It verifies signatures on every message it receives *before* its language model is ever invoked — unsigned input never reaches the AI. And when AI participates in authority decisions, the split is absolute: an advisory proposer may suggest a grant; the deciding verifier is bound by its own trait contract to be deterministic — "no async, no network, no LLM" — and it can only tighten a decision, never loosen one. A shadow verifier runs in parallel and logs every disagreement, an evaluation harness built before the AI is trusted with anything.

The wallet states its red lines in its own documentation: "No address-only login. No wallet-address-derived encryption keys. No arbitrary signing. No app-visible wallet RPC." There is no `sign(data)` operation anywhere — only typed intents that route through human approval. Apps never hold keys; they hold narrowly scoped permission to ask.

The protected-content path — the runtime's rights machinery — is a chain of receipts. Opening a sealed object takes eight steps, and each step is gated by the signed receipt of the one before: the key provider refuses release unless it holds a rights receipt bound to the exact same content, person, session, and right; the decrypt sandbox refuses without a matching release receipt; and the release receipt, by construction, carries zero key material — "a receipt, not a key carrier." Every wire structure rejects unknown fields, so a request smuggling a raw key fails to even parse. The component that briefly sees a live content key is designed to hold the smallest possible authority, use it, and erase it.

And here is the sentence that makes all of the above believable: **every rights, key, and decrypt backend in the runtime today returns `not_configured` — on purpose.** The rights provider's own status string is `fail_closed_until_policy_backend_configured`. No decryption happens in this repository yet, and the code says so about itself, at runtime, in a machine-readable voice. The boundary is proven first. The economics are wired second. That ordering is the discipline.

The accounting is equally plain. About 1,700 automated tests across the core crates as of June 2026, including hash-collision specs and permission-bit assertions. A trusted core of roughly sixteen thousand lines against a five-to-seven-thousand-line target the architecture document itself flags as not yet met — beside a server binary of roughly 145 thousand lines that the end-state design says must move outward. An audit log that is comprehensive, runtime-owned, and append-only — with tamper-evident chaining explicitly labeled "later." Cryptographic envelopes that *require* hybrid post-quantum algorithm listings as policy, while the running ciphers today remain classical. No third-party audit yet; this system can cite the capability-security lineage, not borrow its proofs.

Most systems ask you to trust the adjectives. This one hands you the checklist of what it refuses to do.

---

## IV. Why It Matters

### Property: the largest pool of dead capital ever created

The economist Hernando de Soto showed that poverty is often not a shortage of assets but a shortage of *title*. A house that cannot be deeded cannot be mortgaged, sold at distance, or divided into shares. The asset is real; the capital is dead. His remedy was not new assets but a representation system — registries, deeds, receipts — boring bureaucracy that makes ownership legible and enforceable.

Digital content is the largest pool of dead capital ever created. A song, a film, a dataset, a scan of an actor's face — each is an asset, and almost none of it is capital. A file carries no title. It is infinitely copyable, so it cannot be scarce. It has no enforceable rights attached, so it cannot be licensed without a platform standing in the middle. Its future income cannot be divided, pledged, or sold forward by the person who made it. The creator economy's answer has been tenancy: park the asset inside a platform and accept the rent.

A Digital Capsule is a titling system for digital property, with the bureaucracy implemented in cryptography. Strip the branding and you find instruments any property lawyer would recognize. The deed: a content identifier plus the creator's signature — self-authenticating from any source. The lock: encryption of the payload itself, so the sealed bytes can sit in a public square; access is enforced by rights checks and key release, not by hiding where the file lives. The registry: rights recorded on a chain rather than in a platform's private database. The recorder of deeds: signed receipts at every transfer of authority. And the split: royalty tokens that make a work's revenue division explicit and programmable — a collaborator paid in a recorded share of the work's own revenue rather than in promises, with the division executing itself at every sale.

That last instrument is the quiet revolution. The NFT era gestured at it and failed, because it sold receipts without locks. A Capsule binds the token to the lock. And the doctrine underneath is worth underlining, because it inverts a lazy assumption: **enforcement is not the enemy of openness; it is the precondition of markets.** Mitchell's own analogy is the honest one — your home is private property, and being able to protect it is precisely what lets you rent it out.

One division of labor makes the whole design legible: **availability stores bytes; rights decide who may use them.** Storage becomes a commodity anyone can supply, evidenced by signed receipts. Rights remain property, held by the creator.

### The access economy: what happens to the 45 percent

Why do the platforms' published cuts run from 30 to 55 percent? Not from malice — from function. A platform is a trust factory. It verifies content, custodies it, enforces access, collects payment, splits revenue, adjudicates disputes. The take-rate is the price of manufactured trust, and as long as only a firm can manufacture it, the firm's rent expands to match.

The Capsule architecture moves those functions out of the firm and into the object. Verification: the content proves itself, by hash and signature, from any transport. Enforcement: key release is mechanically bound to a rights check, not to a moderator's mood. Settlement: royalty splits execute inside the purchase itself, instantly, to every named hand. Record-keeping: signed receipts, produced once by code instead of forever by staff. What remains for a marketplace to sell is discovery and experience — real services, thin ones, priced by competition. That is how a 2 percent fee is a business model rather than a subsidy.

Say it plainly: Elacity cannot charge 45 percent, because it no longer performs the functions that justified 45 percent.

### The agent economy: commerce needs a constitution

The strongest argument for this architecture arrived from outside it.

In 2025 and 2026 the industry built payment rails for AI agents — checkout protocols from the largest labs and card networks, crypto rails settling tens of millions of machine payments — while simultaneously demonstrating, in public, that agents cannot be trusted with credentials. Exposed instances by the tens of thousands. Protocol CVEs by the dozen. Nine in ten agents over-permissioned. The rails all assume the agent lives *somewhere* trustworthy, holding keys that mean something. Nobody built the somewhere.

Machine commerce has a specific economic shape. Transaction costs fall toward zero, so volume explodes, so per-transaction human oversight becomes impossible. Which means authorization must become **a fixed cost per grant, not a marginal cost per transaction** — decided once by a human, enforced mechanically a million times. That is precisely what a capability token is.

And when the counterparties are machines, receipts stop being paperwork and become the substrate of liability. An agent with your session cookie cannot be a counterparty. A payment without a rights receipt is a tip, not a license. Accountability without signed audit is a subpoena, not a system. The receipt chain in the runtime — rights receipt, key-release receipt, decrypt session, audit event, each bound to the last — is what a machine-speed license needs to be disputable, auditable, and one day insurable.

To be exact about today: ElastOS does not interoperate with any of the agent-payment rails, and no end-to-end agent purchase runs inside the runtime yet. The rails are context, not integration. The claim is narrower and stronger: the rails built the roads and the money; the authority seat — where the agent lives, holds its keys, and is governed — stands empty. That seat is the product.

### Why now, in three beats

One. Agents arrived at consumer scale — the demand is proven, loudly.

Two. Their credential model failed in public, with numbers attached — exposed servers, leaked secrets, over-permissioned by default.

Three. The incumbents conceded the input side cannot be fixed — prompt injection, in their own words, may never be fully solved. So the only durable defense is bounding what a fooled agent can do: explicit, narrow, revocable, audited, fail-closed authority, at the operating-system level, owned by the person.

We did not manufacture the crisis of rented computing. We were, improbably, already building the answer when it arrived.

---

## V. How We Build

### The law of the house

The repository opens with seventeen principles and calls them "the set of constraints that should decide ambiguous implementation choices." Not a roadmap — a constitution. The spine of it:

**Local first.** "Public exposure is layered on top of local truth, not the other way around." Your machine is the primary world; the internet is an adapter.

**No ambient authority.** Nothing acts because of where it is running. Missing authority fails closed. "Opening a page and holding a capability are different things" — screen position, routes, and DOM presence are never power.

**One canonical path per operation.** No silent fallbacks, no hidden alternate routes. When the intended path is not ready, the system says so instead of quietly downgrading.

**Trust travels with signed content.** Hashes, signatures, and identifiers anchor trust — never gateway locations or host paths. And "encrypted content should be normal, not a special exception."

**Docs, code, tests, and ops must agree.** "The architecture is only real when the repo surfaces teach the same contract. Drift should be treated as a bug." A system that lies about its own state — even by omission, even by soft fallback — is broken.

And when two choices both work, the Decision Rule breaks the tie: prefer the one that strengthens local and content identity, reduces ambient authority, removes hidden paths, keeps the trusted core smaller, and makes the user's mental model clearer. Sovereignty, minimalism, and legibility are the tiebreakers for every ambiguous call.

### Honesty as engineering

Most projects treat honesty as a virtue. This one treats it as a primitive — architected into files, status strings, and CI.

The public state ledger records proven truth only, and reads like a confession: "0.5.0 is a review candidate, not a release tag." "Product Browser completion is not claimed." The marketplace's Install button is honestly disabled, and says why on its face: "Signed install pending." Providers announce their own incompleteness at runtime, in machine-readable voice: `fail_closed_until_policy_backend_configured`. The security file publishes *open* vulnerabilities, for transparency. The checklist culture is explicit: "if a story is not proven, hide or demote the surface instead of overclaiming."

This is not modesty. It is the same property the runtime enforces on software, applied to speech: fail closed, then explain. In a market drowning in vaporware agent operating systems, verifiable self-criticism is the scarcest luxury good — an expensive signal no competitor can fake without first surviving it.

An engineering culture that refuses to lie to its users starts by refusing to lie to itself.

### One law for humans and machines

Principle seven is the charter: "Humans, bots, and AI should not get separate magical trust systems." If humans and agents live under different laws, the agent path becomes the bypass. Symmetry is a security property.

So the authority chain is identical for a person clicking a button and an agent calling an API: principal → verified proof → short-lived session → scoped capability → provider-mediated effect → signed audit. And where the industry lets automation run looser than people, this house inverts it, in five words that should be carved above the door: **automation gets more explicit, never more ambient.**

In practice: an agent is a delegated principal a human creates and can revoke. It has its own name — a DID cryptographically distinct from its owner's, with the ownership recorded where everyone can read it. It signs what it says. It verifies what it hears before its model ever runs. It holds narrower grants than its human, not broader ones. High-risk acts — signing money, exporting recovery material, installing providers — route to a human's explicit approval. Agents never borrow human cookies, never automate a person's real passkey. And the house's own advisory AI may only propose; a deterministic verifier decides, and can only tighten.

Every culture tells the warning story twice: the golem that serves while the true word is written on it, the genie that grants exactly what is asked, catastrophically, because a wish is an unbounded grant. 2026 supplied the modern telling, at scale, with CVE numbers. Our answer is the grammar of the old stories: the servant is named, bounded, and watched — and safe to keep.

If the servant can always be tricked, safety lives in what the servant is permitted to touch.

### The guest room

The oldest law of the house is hospitality. In archaic Greece it was sacred before writing was common: the host owes the guest protection *even from the host himself*.

ElastOS encodes that law in cryptography, and states it as an engineering requirement: "Guest privacy must be real, not courtesy UI." The first passkey on a machine becomes the admin — but guests enroll themselves, and "the admin controls the enrollment policy but does not create or hold the guest's authenticator." Each guest gets their own principal, their own encrypted root, wrapped only to protectors the guest holds. The radical clause follows, quoted with its own exceptions intact rather than improved for effect: the admin may operate the runtime, but should not be able to decrypt a guest's personal root without that guest's explicit recovery, sharing, legal or operator policy, or a future threshold authorization path. The mechanics to make that structural are being built. "Passkey removal revokes access, not storage." And the starkest sentence in the roadmap: "If every protector is lost, encrypted data should be unrecoverable by design rather than silently accessible through a device-global bypass."

Data loss, preferred to backdoors. That is a moral position expressed as key management.

The guest also keeps the ancient right of departure: export your recovery material, migrate your encrypted root to your own machine. The right of exit is what makes staying a choice.

Status, stated plainly: this covenant is design doctrine with partial coverage today. Selected state lives under protected envelopes; some — notably browser-VM profile disks — does not yet, and the repo tracks that gap in public, alongside a live obligation: "keep proving admins never receive guest authenticator, recovery phrase, or principal data-key material." A host who posts his unfinished obligations on the door is practicing hospitality already.

### Economics last, on purpose

The build order is moral, not just tactical: principals, then packages and interfaces, then availability, then protected content, then — only then — economics. The repo's own sequencing rule defers rich DRM economics, token mechanics, and DeFi integrations "after principals, packages, interfaces, availability receipts, and spaces are real." Publishing must mean availability, not just minting an identifier — and payment incentives come only after receipts, quotas, and abuse controls exist. "Do not call a single pinning service decentralized storage."

Economics are the roof, never the foundation. Every crypto-era failure you can name poured the roof first.

---

## VI. The Category and the Position

### The name

Markets file new things under the nearest familiar label, and every label near us is a grave. So we name the ground ourselves:

**The sovereign runtime** — a computing layer you own, where every actor, human or AI, acts on explicit, revocable, audited authority, and where property carries its own lock, rights, and till.

Why "runtime" and not "OS"? Because the name must fail closed too. The repo is pre-release; the browser proof is open; the docs admit the core still carries more than its end-state weight. "Runtime" claims exactly the layer that exists in code — signed capsules, capability tokens, one authority model, three isolation substrates. When the OS is earned, the category grows into it.

And "sovereign" answers the one question every competitor answers wrong: *whose root?* Microsoft — the closest, best-funded neighbor — cannot say this word. Its agent identities root in its own cloud tenant; its agent workspaces are rented by the month. The company with the resources to contest the category is structurally disqualified from its defining word.

### The traps we refuse

Six categories would bury us, and we decline them all. Not a *crypto project* — judge the Rust, not the token; the chain is one provider among many, and passkeys, not wallets, are the front door. Not an *NFT marketplace* — that era sold receipts without locks, and we are the autopsy's answer, not its sequel. Not *DePIN* — the invention here is the authority model, not node incentives. Not a *personal server* — those are real and niche, and increasingly they host agents with no authority model at all. Not an *AI OS* — the label is 2026's loudest vaporware, and our own honesty discipline couldn't survive it. And never, in self-description, a *DRM company* — centralized DRM is the incumbent's word for the incumbent's cage. What we build is creator-controlled rights: the encrypted bytes may be public; the receipts carry no keys; the lock belongs to the maker.

### The enemy

One enemy, named calmly, like a diagnosis: **rented computing.**

Its mechanism is ambient authority — the god-scoped token, the master badge, the agent with your whole life in its environment variables. Its economics are platform feudalism — the 30-to-55-percent cuts the platforms publish themselves, the overnight demonetization, the ban without appeal. Its newest face is the rented agent — your AI, tenant of someone else's cloud.

We do not wage crusades against companies; houses hold ground, they don't march. Microsoft's move validates the category and defines its ceiling. Our position needs one line: they rent your agent a room in their cloud; we hand you the keys to yours.

The other neighbors get one calm line each. Urbit asked people to move to a new world; we secure the one they already live in — passkeys, files, browsers, wallets. Apple keeps your data private on its silicon — under its root and its store. Self-hosted agent stacks put owned hardware under ambient authority; OpenClaw taught that lesson at scale.

### From → to

**FROM ambient authority on rented computers TO explicit authority on owned computers.**

For builders: from credentials to capabilities. For creators: from platforms that hold your work to Capsules where the rights and the royalties are properties of the work itself. For the agent age: from AI as a tenant of a vendor's cloud to AI as a named, bounded member of your household.

### The point of view, in one breath

AI agents arrived at consumer scale and broke the internet's authority model on arrival. The input side can never be fully secured — the incumbents say so themselves. So the only durable defense is bounding what a fooled agent can do. Every incumbent answer draws that boundary inside its own cloud. Your agents become tenants. Your rights become rows in someone else's database. The sovereign runtime is the opposite root: a computer you own. One explicit, revocable, fail-closed law for humans and AI. Property that carries its own rights and pays its own maker.

### One message per audience

Each audience gets one message, one proof, one ask. Resist the urge to show everyone everything.

**The Elastos faithful.** Message: the twenty-five-year dream finally has a working core — Rong Chen's 2000 doctrine is now enforced Rust, not a whitepaper. Proof: the Keystone Gift, and the official line — "for the first time in eight years, Elastos has a working core product." Ask: participate in the DAO mandates; every proposal public, every vote on-chain.

**Creators.** Message: you don't post your work, you keep it — and sell the key, with the rights and the royalties traveling inside the work itself. Proof: the live ela.city pipeline — sealed Capsules, instant royalty splits to a tenth of a percent, a published 2 percent fee against the platforms' published 30 to 55. Ask: publish one piece of protected work this week.

**Developers.** Message: capabilities, not credentials — deny-by-default in the lineage of the great capability systems, extended to a whole personal computer, agents included. Proof: the code — twelve checks per use, every failure audited, about 1,700 tests, delegation that can only narrow. Ask: clone the repo, run the smoke tests, read the principles — then try to find where we lied.

**Agent builders.** Message: OpenClaw proved people want a personal agent; we make wanting it survivable. Proof: the agent capsule — own DID, signed speech, verification before inference, scoped grants, human approval on high-risk acts. A compromised agent here has a blast radius the size of its token, not the size of your life. Ask: run the agent against a local model and read its audit trail.

**Investors.** Message: Build 2026 validated OS-level agent authority; we are the self-custody counterpart, holding the one intersection no one else holds — owned substrate, one human/AI authority model, native rights and payments, peer-to-peer transport. Proof: a shipping cadence you can audit commit by commit, DAO-funded runway, and a documentation culture that makes every claim checkable. Ask: a working session on the hosted runtime, with the state ledger open in the next tab.

**Mainstream creators and families.** Message: your things, in your house, under your key, reachable from any screen you trust — and money that arrives without a landlord. Proof: the fee schedule is public — Elacity keeps 2 percent where the platforms' published cuts run 30 to 55 — and royalties arrive in the instant of the sale. Ask: a five-minute onboarding — upload one file, watch the agent fill in the listing. Never "join Web3." Sell outcomes; sovereignty is the mechanism, never the pitch.

---

## VII. The Language

The codebase already has a voice — plain, exact, quietly stubborn. "Fail closed, then explain." "Make Home a boring front door." The writing must sound like the code, because the whole promise is that the words and the system tell the same truth.

### The voice, in five rules

**1. Short old words.** Key, lock, own, house, rules, keep, prove, refuse. Not "leverage," not "utilize," not "ecosystem synergies." When a Latin word and a Saxon word compete, the Saxon word wins.

**2. One picture per idea, and the picture never changes.** A *key* is a capability. A *receipt* is an audit record. A *sealed package* is a Capsule. A *permission slip* is an agent's grant. The *house* is your runtime. A *tenant* rents; an *owner* holds the deed. Every abstraction in the stack maps to one of these, and the mapping is permanent. Where the mechanism ends, the poetry stops — a metaphor may describe direction only when it is flagged as direction.

**3. Short sentences. Declarative.** The system fails closed; so do our sentences. If a sentence needs a semicolon to survive, it is two sentences.

**4. Zero crypto jargon by default.** Blockchain, token, DID, CID, DRM — these appear only for audiences that already use them, and even then after the plain version. The test: would a filmmaker, a teacher, or a fourteen-year-old know what we mean? The origin story — an actor told the studio owned his own face — needs no jargon. Neither does the product.

**5. Quiet confidence.** No exclamation marks. No "revolutionary." People who have the goods don't shout. The strongest sentence in the canon is understated: "For the first time in eight years, Elastos has a working core product."

### Master lines

The roof line, then the catalog. Each line earns its place by being true today or marked as direction.

- **A computer that answers to you.** — The roof. Homepage, keynote close.
- **Own the room your AI works in.** — The agent-era master line.
- **Your work. Your keys. Your terms.** — Creator-facing roof line for Elacity.
- **Your things, in your house, under your key.** — The object model for ordinary people.
- **Reachable from any screen. Owned from exactly one place.** — The personal cloud, said without the acronym.
- **Nothing moves without a key. Every key leaves a receipt.** — Capability plus audit, for anyone.
- **A key, not a badge.** — The whole capability doctrine in five words.
- **No ambient internet.** — Developers and security audiences; it is literally the doctrine's own phrase.
- **The AI may propose. Only deterministic code decides.** — Devastating because it is quotable *and* compiled.
- **Automation gets more explicit, never more ambient.** — The inversion of the industry default.
- **The content is the identity — not the address.** — Content addressing without saying CID.
- **Fail closed, then explain.** — The engineering brand, three words and a comma.
- **We publish what is not done yet.** — The honesty signature, stated as a promise.
- **The lock, the rights, and the till travel with the work.** — Capsules, for creators and press.
- **They sold receipts without locks. A Capsule is the lock, the rights, and the payment in one object.** — For audiences digesting the NFT winter.
- **The browser is an app here, not the landlord.** — The inversion, for consumers.
- **Your AI works for whoever holds its credentials. So hold them.** — Talks, threat-model writing.
- **They rent your agent a room in their cloud. We hand you the keys to yours.** — The Microsoft sentence.
- **Judge the Rust, not the token.** — The crypto-baggage sentence.
- **Sovereignty should feel boring.** — Design philosophy; pairs with "Make Home a boring front door."
- **The internet made you a tenant. This is the deed.** — Manifesto register. Use sparingly; it must be earned by the honesty around it.

### The elevator

**Ten words.** A computer you truly own — where even AI needs permission.

**Thirty words.** ElastOS is a sovereign runtime — a computing layer you own — where every app, browser, and AI agent acts only on explicit, signed, revocable permission. Your files, keys, rights: yours. The proofs are published.

**One hundred words.** Every platform you use holds your keys: your files, your logins, your audience, and now your AI's credentials. Elacity builds the alternative — ElastOS, a sovereign runtime where authority is never ambient by design: every app, browser session, and AI agent acts only on an explicit, signed, auditable, revocable permission, under the same rules for humans and machines. Alongside it, Elacity already runs a live marketplace where creators seal work into Capsules and sell the keys directly — today on Elacity's existing rights stack, with the runtime's stricter fail-closed pipeline built to receive it. The security core is built, open, and tested. The rest we publish honestly: including what is not done yet.

**Three hundred words.** In 2026 the industry admitted two things. AI agents at consumer scale hold catastrophic ambient credentials — exposed servers by the tens of thousands, leaked keys, thirty-plus CVEs against the standard agent-tool protocol in two months, all of it public record. And the input side can't be fixed: OpenAI's own security chief says prompt injection may never be fully solved. If an agent can always be tricked, the only durable defense is bounding what a tricked agent can do.

That is what Elacity builds. ElastOS is a sovereign runtime — the core of a personal operating system — with one law: no ambient authority. Every app, browser session, and AI agent acts on cryptographically signed, narrowly scoped, revocable permissions — checked twelve ways on every use, with every use and every refusal logged. Humans and AI agents live under the same law; an agent gets its own identity, owned by a person, and its permissions are more explicit than a human's, never more ambient. The AI may propose; only deterministic code decides. This core is real today: open source, running on three isolation substrates, with about 1,700 automated tests.

Alongside the runtime, Elacity runs a live marketplace, ela.city, where creators encrypt work into Capsules and sell access directly — rights, royalties, and resale rules traveling with the work itself, recorded and settled on-chain. It runs on Elacity's existing rights stack today; the runtime's fail-closed pipeline is being built to receive it. The founder spent years digitizing actors for Hollywood studios and watched them learn the studio owned their own likeness. This is the answer he went off to build.

The idea is older than us: Rong Chen left Microsoft in 2000 to build an OS where apps never touch the network directly. Twenty-five years later, the pieces finally exist. We ship them in the open, fail closed where we aren't finished, and publish what is not done yet.

**ela.city, standing alone.** A marketplace where creators sell keys, not copies — rights and royalties travel with the work, splits pay in the instant of sale, and the published fee is 2 percent.

**Elacity Labs, for press and hiring.** Elacity Labs builds ElastOS and ela.city — funded in public by on-chain community mandates, paid against delivered work, shipped fail-closed.

### The system of names

One rule above all, from the principles themselves: one visible concept, one primary name. A newcomer should meet at most two proper nouns in their first five minutes: **Elacity** and **ElastOS**.

- **Elacity** — the brand people meet first: the marketplace (ela.city) and, by extension, the mission.
- **Elacity Labs** — the company. Corporate, hiring, governance, and press contexts only. Never in product UI.
- **ElastOS** — exactly two capitals — the operating system product. "ElastOS by Elacity Labs" in formal contexts.
- **Elastos** — one capital — the broader twenty-five-year ecosystem: the Bitcoin-merge-mined chain, the DAO, Rong Chen's lineage. Historical and ecosystem contexts only. Lowercase `elastos` is the binary and the URI scheme; developers only.
- **ELA** — the Elastos ecosystem's native asset. Ecosystem and investor contexts only, and always by function: it settles the Elastos chains and denominates the DAO treasury that funds this work. Its price is never discussed. Anywhere. Ever.
- **Home** — what users see when ElastOS opens. Always the plain word, capitalized like a place. Its siblings stay human: Library, Documents, Apps, Marketplace, Messages, People.
- **Apps vs. capsules** — the load-bearing register rule: "Apps" is the public word; "capsule" is the internal and developer word. A user installs and opens Apps. A developer builds and signs capsules.
- **Capsule, creator sense** — on Elacity, a **Capsule** (formally, Digital Capsule) is a creator's sealed work: content, lock, rights, and payment rules in one object. This is the only public use of "capsule," and it earns it — it is genuinely a sealed thing you can hold, trade, and open with a key. Guard the collision: creator documents say Capsule; developer documents say capsule; no document uses both meanings without a one-line note.
- **Carrier** — developer and architecture term for the peer-to-peer plane. Publicly: "the private network between your devices and your people." Never lead a consumer sentence with it.
- **PC2** — internal and ecosystem shorthand for the Personal Cloud Computer idea and the Puter-fork lineage. In public writing, spend the idea, not the acronym.

The introduction ladder: first contact — Elacity and ElastOS, plain words only. Second — Home, Apps, Capsule (creator sense). Third, for developers — capsules, providers, capabilities, principals, Carrier. Fourth, for the ecosystem — Elastos, ELA, DAO, merge-mining, DIDs. Never skip rungs. Anyone forced to learn "capability token" before they have felt "your things, in your house, under your key" was handed the ladder upside down.

### Forbidden language

Using these is a bug, and gets fixed like one.

**Hype words, banned outright:** revolutionary, game-changing, paradigm, disruptive, cutting-edge, next-generation, seamless, frictionless, military-grade, unhackable, unbreakable, bulletproof, world-class, "the future of X," "10x."

**Category words, banned by default:** "Web3" as identity (we are not "a Web3 OS"); "blockchain-powered" as a lead; "trustless"; "NFT" (say tokenized access and rights, explain the mechanics, skip the word); "DRM" unqualified (say creator-controlled rights); "metaverse"; "decentralized equals secure" (the agent crisis falsified it in public).

**Overclaims, banned as false today:**
- "Post-quantum encryption." We have crypto-agile envelopes that *require* post-quantum algorithm listings as policy; no post-quantum cipher runs yet. Say: designed for the post-quantum migration.
- "Tamper-proof audit log." The audit plane is comprehensive and runtime-owned; cryptographic chaining is explicitly "later."
- "Production dDRM" or "decentralized key management is live." The runtime's rights path fails closed by design; backends are not configured. Today's live marketplace rights run on Elacity's earlier stack.
- "Even the admin can't read your data," stated as shipped fact. It is the design doctrine, with partial coverage. Say: designed so that.
- "Install apps from the marketplace." Install is honestly disabled — "Signed install pending."
- "A finished consumer OS." The runtime is a pre-release review candidate; its README says not for production.
- "First ever" anything, without adversarial verification. "No competitors" — Microsoft announced theirs on stage.
- "Users demand sovereignty." Users demand outcomes. Every sovereignty-maximalist OS before us starved proving it.
- Unreconciled numbers stated as hard fact — revenue-share percentages, the podcaster's exact earnings. Reconcile first or say "roughly," with the source.
- Any talk of token price, in any form, ever.

**Structural bans:** no exclamation marks. No feature in the present tense unless it passes its own proof today. No roadmap item without a "direction" marking or a named proof path. And never delete the "not yet" section to make a page prettier.

---

## VIII. What Is True Today

*(This section is the credibility engine of the whole book. It is reconciled against the repository's own ledgers as of mid-2026; where this book and the ledger ever disagree, the ledger wins. One bullet reconciles against Elacity's published materials instead of this repository, and says so on its face. Read the middle column carefully — most companies hide it. We ship it, labeled, with its own status strings.)*

### Works today — proven

- **The capability core.** Ed25519-signed capability tokens bound to a specific capsule; twelve validation checks in strict sequence on every use; every failure emits an audit event; enforcement wired at the microVM bridge, the HTTP handlers, and the shell protocol. Revocation by epoch and by token. Atomic use-counting, proven race-free under a twenty-way concurrency test. Delegation that can only narrow, never re-delegate.
- **Three isolation substrates, one contract.** WebAssembly with fuel metering and bounds-checked host calls; Linux microVMs that refuse to launch without hardware virtualization rather than degrade; macOS microVMs through a hand-written binding to Apple's virtualization framework (proven today for browser-VM workloads, not yet at full parity as a general capsule substrate).
- **Passkey-fronted accounts.** A from-scratch, minimal WebAuthn implementation; credentials encrypted at rest; device identity as a self-certifying DID; recovery kits that wrap the user's data key to a phrase. Wallets and DIDs attach as proofs on the principal — never as the login itself.
- **A working agent under the law.** A persona DID cryptographically distinct from its human owner's, with ownership recorded; every outbound message signed; every inbound message verified before the language model runs; scoped grants narrowed by mode.
- **The policy split.** Advisory proposer, deterministic verifier ("no async, no network, no LLM"), tighten-only decisions, and a shadow verifier whose disagreements are audited.
- **The wallet's red lines.** No address-only login, no arbitrary signing, no app-visible RPC, no `sign(data)` — typed intents behind fresh passkey approval, with signed receipts.
- **Manifest law.** Ordinary app capsules cannot even request raw networking or provider powers; provider capsules must declare their authority, operations, and expected audit events in their signed manifests.
- **Signed publish, install, and update.** The command-line flow works today over configured trusted sources — the README lists it under "What Works Today." What remains disabled is the Marketplace UI's one-click Install, pending end-to-end verification of signed manifests, publisher identity, and receipts.
- **A live marketplace (external surface).** ela.city operates today, per Elacity's published product materials: work encrypted into Digital Capsules, access sold from one key to billions, royalty tokens splitting revenue instantly down to a tenth of a percent, distribution rights for resale, channels and subscriptions, a published 2 percent fee — running on Elacity's existing rights stack, outside this repository. This bullet reconciles against Elacity's materials, not this repo's ledgers.
- **The proof culture itself.** About 1,700 automated tests across the core crates (as of June 2026); a public state ledger of proven truth; published open findings; and a smoke test that drives the hosted runtime through the real Home passkey ceremony — exercised with a WebAuthn virtual authenticator, the same protocol path CI uses — to open a known protected title on ela.city, proving the journey surface end to end (today's decryption is performed by ela.city's own stack).

### Built, but fail-closed or partial — in progress

- **The runtime rights pipeline.** The eight-step protected-content contract exists and is enforced — receipt chained to receipt, no key material in any receipt, unknown fields rejected on every wire structure — and every rights, key, and decrypt backend deliberately returns `not_configured`. No sealed-content producer, no live key release, no runtime decryption yet. The boundary first; the economics second.
- **Marketplace installs.** Browsing, trust ledgers, and launching work; the Install button is honestly disabled — "Signed install pending" — until signed manifests, publisher identity, and receipts are verified end to end.
- **On-chain purchase through the runtime.** A funded live purchase and playback has been proven once, on the known ela.city test path through Runtime Browser, against live contracts on Base. One blocker stays open in public: the buy executes on-chain and unlocks content, but the dApp still displays a failure for a return-path reason — the task ledger tracks it. No arbitrary buy-and-trade readiness is claimed.
- **Protected roots.** Principal-root encryption covers selected state; browser-VM profile disks are explicitly not yet under the protected envelope, and the receipts say so: `principal_owned_reset_scoped_unprotected`.
- **The audit plane.** Comprehensive, runtime-owned, append-only — with tamper-evident cryptographic chaining an explicitly documented "later."
- **The trusted-core diet.** Roughly sixteen thousand lines of core against a five-to-seven-thousand-line target, beside a server binary of roughly 145 thousand lines that the architecture says must move outward. The repo audits its own waistline.
- **The browser.** Runs as a proof; fails its own product bar — audio proof and hash-bound UX evidence are open blockers, and the current baseline is labeled, in its own receipts, "managed baseline, not final product."
- **Guest privacy.** Doctrine plus partial mechanics, with a standing public obligation: keep proving that admins never receive guest authenticators, recovery phrases, or data keys.
- **Known open findings, published.** Chat signatures lack replay protection; host-plane infrastructure services receive empty capability tokens by documented design. Listed in the security file, for transparency.

### Direction — declared, not built

- **Post-quantum cryptography.** Envelopes *require* hybrid classical-plus-post-quantum algorithm listings as enforced policy; no post-quantum cipher executes yet. The honest phrase: crypto-agile, designed for the migration.
- **The decentralized key network (dKMS).** Threshold nodes, share ceremonies, the sealed decrypt-material envelope — specified, not running.
- **Home as a full object browser**; mounted third-party spaces; the cloud-provider bridge.
- **Ecosystem identity adapters.** Resolver-backed `did:elastos` verification, DID-only recovery.
- **Carrier lineage interop.** Today's transport is iroh; native Elastos Carrier and Boson backends are stated targets, not started.
- **The lineage map, for the faithful.** Native Carrier and Boson are honored transport targets, as above. The Elastos Smart Chain and EID sit among the chain adapters, beside Base. Legacy Hive storage has no adapter in the new map today — objects, availability receipts, and spaces are the runtime's own storage story — and if that changes, it will arrive as a provider, labeled.
- **Agent-payment rails.** No MCP, x402, AP2, or ACP integration exists; the rails are market context, and the empty seat beside them is the strategy.
- **Deferred on principle.** Rich rights economics, token mechanics, DeFi and BtcFi, storage markets — sequenced after principals, packages, availability, and receipts are real.
- **Adoption metrics.** None exist publicly — no volumes, no user counts — and we will publish none until they are real, with the same receipts discipline as everything else.

Early is the honest word.

*A note on sources. Claims about the runtime reconcile against this repository — code, tests, ledgers, status strings. Claims about ela.city reconcile against Elacity's published materials. Ecosystem dates, votes, and funding reconcile against the DAO's public on-chain records. Founder biography follows the founders' own public accounts, and says so in the text. Numbers from outside — platform take-rates, star counts, exposure scans, CVE tallies, survey percentages — are reported figures from published terms, public registries, and named security research; each carries its qualifier where it stands, and a number we cannot pin, we cut.*

---

## IX. The Road

*(Everything in this section is direction, and says so.)*

The sequence is already written into the project's planning gates, and it is a morality as much as a plan:

1. **Authority first.** Passkey-first accounts, human-created and human-revocable agent principals, delegation with human approval on high-risk scopes.
2. **Availability second.** Publishing that means *available*, evidenced by signed receipts — never a bare identifier and a hope.
3. **Protected content third.** The sealed-object pipeline wired for real: sealed publish, on-chain rights reads against pinned contracts, key release against receipts, decryption inside the smallest possible boundary — replacing today's compatibility stack step by step, fail-closed at every unfinished edge.
4. **Adapters fourth.** Wallet, DID, and chain proofs attached where identity and settlement genuinely need them.
5. **Spaces fifth.** Network drives and shared places that make Carrier a real object plane and Home a real object browser.
6. **The signed registry sixth.** Publish, install, update — which is the day the marketplace's Install button turns on, because its promise can finally be kept.
7. **Economics last.** Markets for storage, distribution, and rights — after the receipts, quotas, and abuse controls that make them honest.

Three lanes run beside the gates, each as much direction as the gates themselves.

**The market's road.** ela.city keeps running on its current rights stack for as long as creators depend on it — continuity first. The cutover to the runtime pipeline happens surface by surface, and only behind fail-closed parity proofs: sealed publish, rights reads, key release, and playback must each pass on the runtime path before any creator's flow moves. What a creator should feel when it happens: nothing, except more receipts.

**The product line's road.** The shipped PC2-lineage product converges on the runtime by the written rule — inherit the protocol boundaries and the acceptance tests, never the monoliths. The convergence is observable, not atmospheric: surfaces re-homed onto runtime services one at a time, each behind its acceptance tests, while the oversized server binary sheds weight in public. The repo already audits its own waistline; this road is that audit trending toward its target.

**The workshop's road.** Labs' mandate cadence stays public — proposal, vote, delivery, tranche — and the first third-party security audit of the capability core is a stated milestone, not a settled fact. Until it lands, this system cites the capability-security lineage; it does not borrow its proofs.

Three futures, held as scenarios rather than prophecies. The bear: the platforms absorb the lesson, agent tenancy becomes "good enough," and this work becomes the reference the absorbers copy — correct, admired, niche; even then it compounds. The base: agent incidents keep arriving on schedule — the input side is conceded, so they will — and a meaningful minority of creators, professionals, and self-hosters adopts owned authority the way businesses adopted firewalls: not from ideology, from insurance logic. The best: one machine-commerce failure with a dollar figure large enough to name makes "where does your agent live, and who can prove what it may do?" a compliance question — with exactly one self-custodial answer.

Discipline for all three: claim the gradient, not the demand curve. People do not want sovereignty; they want the file to be theirs, the sale to settle, the agent to act without becoming the next breach headline. Sovereignty is how. It is never the pitch.

And one picture of where the road points, marked as the picture it is. A filmmaker's agent, some year soon, is approached by a fan's agent asking to license a scene. Policy checks policy at machine speed; the rights are already bound to the sealed work; the split pays every named hand in the same breath as the sale; a chain of signed receipts records the grant, the release, and the render; either human can inspect, dispute, or revoke — and neither was interrupted at dinner. Every noun in that paragraph has a schema, a contract, or a fail-closed skeleton in the repository today. None of it is wired end to end. That is exactly what the middle column of Section VIII is for — and why it exists in public.

---

## X. Benediction

May your keys open exactly what they say, and nothing more.
May every grant expire, every receipt survive, and every refusal explain itself.
May your work travel with its lock, its rights, and its till — and come home paid.
May your servants be named, your guests be safe even from their host, and your house be boring the way strong things are boring.

We owned our first computers.
We intend to own our last ones.

The internet made you a tenant.
This is the deed.

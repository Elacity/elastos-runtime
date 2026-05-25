# ElastOS — A 30-Minute Read on What We're Actually Building

*A product overview for humans. Strategy, architecture, and the journey, all in plain English. Reading time: ~25 minutes.*

---

## TL;DR

- **The problem.** Your "personal" computer isn't personal anymore. Your data, identity, messages, money, and AI all live on someone else's servers. When that company changes the rules — or dies — your digital life breaks.
- **What we're building.** An operating system that runs on hardware you already own, gives you back ownership of your data and identity, lets you talk peer-to-peer to other people without going through any corporation, and runs apps in safe little sandboxes so you stay in control.
- **Where we are.** It already works on Linux. As of this week, the front-door experience — the "Home" dashboard — launches on a Mac with one bootstrap command. Nine engineering phases of work brought us here.

---

## Part 1 — Start at the bottom: what *is* a computer?

Strip away the words. A computer is a piece of metal that does three things:

1. **Stores stuff** — your photos, messages, files.
2. **Does math on that stuff** — writes documents, runs games, plays videos.
3. **Talks to other computers** — sends messages, downloads things.

That's it. Three jobs. Storage, compute, network.

Forty years ago, the metal in your house did all three. Your data lived on your floppy disk. Your computer ran your software. Your modem dialed somebody else's modem directly.

Today? Almost none of that is still true.

- Your **storage** is in Google Drive, iCloud, or Dropbox.
- Your **compute** runs in AWS, Azure, or OpenAI's data center.
- Your **network** goes through Meta, Apple, X, Discord — every message read by some middleman.

You don't own a computer anymore. You own a *window* into someone else's computer. They could change the locks, raise the rent, evict you, or close shop tomorrow — and your digital life ends with them.

**This is not how it has to be.** That's the first principle ElastOS is built on.

---

## Part 2 — What ElastOS is, in one breath

> **ElastOS is a runtime that turns hardware you already own into a sovereign personal computer — one that stores your data on your own disk, runs your own apps in safe sandboxes, identifies you with a key only you hold, and talks directly to other people's computers without any company in the middle.**

In a picture:

```
                         YOU
                          │
             ┌────────────┴────────────┐
             │   Home (front door)     │   ← what you see and use
             ├─────────────────────────┤
             │   Apps (capsules)       │   ← chat, AI, notes, sites
             ├─────────────────────────┤
             │   Identity + Network    │   ← your key, peer messages
             ├─────────────────────────┤
             │   Storage (localhost://) │  ← your filing cabinets
             ├─────────────────────────┤
             │   Substrate (microVMs)  │   ← safe rooms for each app
             ├─────────────────────────┤
             │   Your hardware         │   ← Mac, Linux box, Pi, …
             └─────────────────────────┘
```

The whole stack lives on **your** machine. There is no "ElastOS Inc." server you have to ping. There is no account you have to sign into. There is no terms-of-service you have to accept.

---

## Part 3 — The architecture as a house

Imagine your computer is a small house. ElastOS lays it out like this:

### The land — your hardware
A Mac, a Linux laptop, a Raspberry Pi, eventually a phone. ElastOS doesn't care what brand. If it's metal and electrons, it works.

### The foundation — Linux kernel
Even on a Mac, our apps run inside a tiny Linux kernel. Linux is the most-tested operating-system core in history; we trust it to be the slab the house sits on. Apple's macOS or your laptop's hardware drivers are just the *land underneath* — the house itself is the same on every plot.

### Locked rooms — microVMs (the substrate)
Every app gets its own locked room called a **microVM** (a tiny virtual machine). A bad app — or a hacked one — can trash its own room but can't break into yours or anyone else's.
- On Linux, the rooms are made by **crosvm** (Google's tiny VM monitor).
- On Mac, the rooms are made by **Apple's Virtualization.framework** (built into macOS for the same purpose). *Bringing this online is what most of our last nine phases were about.*

Either way, the app inside can't tell what platform you're running. Same room. Same rules.

### The house key — your identity (a "DID")
A **DID** ("decentralized identifier") is a cryptographic key only you hold. It's the digital equivalent of a physical house key:
- Nobody can copy it.
- Nobody can issue a new one for you.
- Nobody can take it back.

When you message someone, post a file, or sign something, your DID is what proves "this came from me." You can have lots of DIDs (one for work, one for play). Apple, Google, and Meta all want to *be* your identity — ElastOS hands the key back to you.

### The mail tubes — Carrier (the network)
A peer-to-peer messaging layer. Picture a personal pneumatic-tube system between your house and your friend's house, with no post office in between. Messages are end-to-end encrypted. Nobody reads them but the two of you.

### The filing cabinets — `localhost://` (storage)
Every piece of information has a name like `localhost://MyWebSite/index.html` or `localhost://Users/sash/notes/today.md`. Apps ask the operating system for those files — they never get raw disk access. Your data layout is sovereign and inspectable.

### The hallway and front door — Home
The "Home" dashboard is what you see when you turn on ElastOS. It shows:
- Who you are.
- What's running.
- What needs your attention.
- Where to go next (Chat? Your website? Updates?).

A real working Home dashboard now launches on Mac. That's this week's milestone.

### The guests — capsules (apps)
Each app is a "capsule." Capsules come in two shapes:
- **Linux capsules** — full programs running inside a microVM. Heavy but fully capable.
- **WASM capsules** — sandboxed code (the same kind of code that runs in your web browser, called WebAssembly) running directly in the runtime. Light and fast.

When a capsule wants something — your microphone, your contacts, your wallet — it has to *ask* you. You see the request, you say yes or no. No silent data slurping.

---

## Part 4 — What we've actually built

Here's the journey, distilled into ten plain-English milestones. No phase numbers — just what changed and why it mattered.

1. **We defined the shape.** Drew up every primitive a sovereign operating system needs: identity, storage, network, sandboxing, app format, distribution, updates. Decided what was in scope and what was not.
2. **We picked the substrates.** crosvm for Linux. Apple's Virtualization.framework for Mac. One Rust trait that abstracts both, so the rest of the code never has to know which platform it's on.
3. **We built the Mac substrate.** Wrote a Rust crate that talks to Apple's framework directly through FFI (foreign-function interface). No middleware, no shims — just calls into the Mac kernel.
4. **We made it run a real Linux kernel.** Boot a real Ubuntu kernel inside a Mac VM, watch the kernel logs fly past, land at userspace handover in under a second.
5. **We booted Ubuntu to a login prompt.** Full systemd, full userspace, on a Mac, in six seconds wall-clock. The same Ubuntu the cloud runs.
6. **We made the console interactive.** Drop you straight into a real root shell on the Mac terminal. Ctrl-C works. Terminal restore works. It feels like SSH but it's a fresh VM that didn't exist a second ago.
7. **We ran a real ElastOS app.** A WebAssembly capsule (the shipped `home` capsule) launches and exits cleanly on a Mac. Along the way we discovered and fixed macOS's "Hardened Runtime" silently killing our WebAssembly engine, by adding the right Apple entitlements.
8. **We hardened the signing path.** Found the right Apple entitlements (the ones that let us boot VMs *and* run JIT-compiled code) and baked them into our dev-signing flow so re-builds don't lose them.
9. **We wrote the operator's playbook.** Documented the recipe — what to fetch, how to sign, what blocks a boot, how to debug a real kernel panic — so the next contributor doesn't have to rediscover any of it.
10. **We lit up the front door.** This week: one bootstrap command builds and registers the three trust-prerequisite providers, and the full ElastOS Home dashboard launches on a fresh Mac source checkout — the same surface Linux users see.

That last milestone is the headline. It means the Mac isn't a "kind of works" platform anymore. It's a first-class home for ElastOS, on par with Linux.

---

## Part 5 — What makes ElastOS special (the unique selling points)

### 1. Sovereignty by default
**Claim:** Your data, identity, and compute live on your hardware. Not "encrypted on our servers." Not "synced to your account." On your disk.

**Why no one else has it:** Every Big Tech operating system makes money by being the middleman. Apple charges 30% on the App Store. Google sells your attention to advertisers. Microsoft sells your AI prompts to Bing. None of them can offer sovereignty — it would kill their business model.

**What it costs:** You run a small background daemon on your computer (no heavier than running Dropbox or Spotify). And right now, no marketing budget — you find ElastOS because you went looking.

### 2. Peer-to-peer by construction
**Claim:** Your computer talks directly to other people's computers. No central message broker. No login server.

**Why no one else has it:** Peer-to-peer networking is genuinely hard — NAT traversal, key exchange, message ordering, offline-first delivery. We built it (the "Carrier" layer). Nobody running a profitable product *wants* to build it. Every Big Tech app *needs* you to go through their broker so they can monetize the traffic.

**What it costs:** You're not always reachable the instant somebody messages you (peers come and go). That's not a bug — it's the human pace of communication. Carriers can also broker messages for offline peers.

### 3. App isolation that's *real*
**Claim:** A bad app can't read your other apps' data. Period. Each one lives in its own microVM.

**Why no one else has it:** Phone OSes pretend to do this with "sandboxes" that all share one kernel. Those sandboxes leak constantly — every iOS or Android jailbreak you've ever heard of is one of these leaks. We use real hardware virtualization. A break-out from one app gets you *nothing* you didn't already have.

**What it costs:** A few hundred megabytes of memory per app. Cheap on modern hardware.

### 4. Capability-based prompts (not "all or nothing" permissions)
**Claim:** When an app wants something (mic, contacts, your AI, your money), it asks. You see the specific request. You decide. The OS enforces it.

**Why no one else has it:** Today's permission prompts are basically "yes/no to all microphone access, forever." Ours can say "yes to this app, this purpose, for the next hour." Capability-based security is a 50-year-old academic dream that finally has a production-quality home.

**What it costs:** You see a few more prompts in the first hour of using a new app. They taper off as the system learns your defaults.

### 5. Same OS, every device
**Claim:** Whether you run ElastOS on a Linux laptop, a Mac, a Raspberry Pi, or — soon — a phone, your apps and data look identical. They are *byte-for-byte* identical, because they're WASM or Linux binaries running in identical sandboxes.

**Why no one else has it:** Apple owns iOS+macOS, Google owns Android+ChromeOS, Microsoft owns Windows. Each is incompatible with the others by design — that's how the lock-in works. ElastOS is the same on every platform because we wrote the layer that makes platforms invisible.

**What it costs:** Performance is a hair slower than running directly on metal. In practice, you won't notice.

---

## Part 6 — Jobs To Be Done: ten ways people would actually use this

Real people don't "use an OS." They *do things.* Here are ten things ElastOS does, told as "I want to ___":

1. **"I want to chat with my friends without Meta reading my messages."** ElastOS's `chat` capsule sends encrypted messages directly over Carrier. No server-side data, no ads, no "we'd like to share your data with our partners."

2. **"I want to publish a website that's mine forever."** `elastos site stage` turns any folder into a sovereign site at `localhost://MyWebSite`. `elastos share` gives it a temporary public URL when you want one. The site lives on your hardware. You can take it offline anytime.

3. **"I want a personal AI that knows me deeply, without my data going to OpenAI."** The `llama-server` capsule runs a real language model on your hardware. The `system` and `documents` capsules let it read your notes, your calendar, your code — without those things ever leaving your machine.

4. **"I want a private group for my family, team, or D&D table."** The `chat-room` capsule lets you spin up rooms identified by a key only you control. Members join over peer-to-peer Carrier. No Discord server can ban it. No Slack subscription required.

5. **"I want to write notes that I'll still have in twenty years."** The `notepad` and `library` capsules write Markdown files into your `localhost://` filesystem. Even if ElastOS itself disappeared, the files are right there on your disk — readable by any text editor.

6. **"I want a 'cloud drive' without the cloud."** Carrier syncs files between your devices peer-to-peer. Two laptops with the same DID can share folders without ever talking to a server.

7. **"I want to run a Linux app on my Mac without spinning up Docker or Parallels."** `elastos run ubuntu-base` boots Ubuntu in six seconds, drops you at a root shell, and tears down cleanly when you exit. Hardware-isolated, no licensing dance.

8. **"I want to share a file with my friend without uploading it to Dropbox."** `elastos share <file>` returns a peer-to-peer link. The recipient gets the file directly from your computer when they're online.

9. **"I want my house's IoT devices to talk to each other without sending data to a Chinese server."** ElastOS on a Raspberry Pi can be the local hub — devices connect to it, it speaks Carrier to your phone, your data never leaves your house.

10. **"I want to keep working when the internet is out."** Because Carrier is peer-to-peer, ElastOS devices on the same wifi (or even Bluetooth, or LoRa radio) can keep talking when the internet's down. The apps don't break — they queue messages for when connectivity returns.

---

## Part 7 — Where ElastOS fits in the world

**The cloud is a trillion-dollar tax.** Apple, Google, Amazon, Microsoft, and Meta together generate over a trillion dollars a year — much of it by sitting between you and your data. Every photo upload, every backup, every message, every AI query — they take a cut. That tax is invisible to most people because it's paid in attention and data, not in cash. But it's real. And it grows every year.

**ElastOS is a quiet defection from that tax.** It's not anti-cloud. (You can still use the cloud for what it's good at — syncing across timezones, running massive compute jobs.) But it *inverts the default.* The default becomes: your data is yours, your apps are yours, your identity is yours. The cloud is something you *opt into* for specific tasks, not something you *opt out of* by default.

We're not the only people working on sovereignty. Tor, Bitcoin, IPFS, Mastodon, Matrix, Nostr, Holochain — all sibling projects. What's different about ElastOS is that we're trying to give you *the whole computer* back, not just one slice (just the network, just the money, just the messages). One operating system, one identity, one storage layer, one app format. Coherent.

The closest historical analogy is **Linux in the 1990s** — a sovereign alternative to Microsoft Windows that nobody took seriously at first, until they did. ElastOS is trying to be Linux for the 2030s. But where Linux was "you can install your own server," ElastOS is "your laptop *is* the sovereign server."

---

## Part 8 — What we have NOT built yet (the honest list)

- **Distribution is still hand-rolled.** We have a stamped-installer pattern, but no notarized macOS installer, no signed Windows installer, no app store. Today's install path is a script you run from a terminal.
- **No formal security audit.** The code is open source. It has not been third-party-audited yet. For a sovereignty-focused OS, this is the biggest gap.
- **The app catalogue is small.** Today: home, chat, chat-room, notes, library, GBA emulator, and a handful of providers. Compare to the App Store's millions. We are at version 0.2.
- **No multi-user accounts yet.** One Mac user account = one ElastOS home. Family sharing, kids' accounts, work/personal separation — all future work.
- **Mobile is missing.** ElastOS doesn't run on iPhone or Android yet. Apple makes this very hard — iOS doesn't allow real virtualization. Android is doable but unstarted.
- **Hardware integration is shallow.** No proper webcam access from capsules yet, no smart-card identity, no hardware-wallet integration.
- **No browser engine of our own.** When a capsule wants to render HTML/CSS, it leans on the host's browser. Long-term we want a sovereign browser too.

We list these because they're real, and we're going to fix them one by one.

---

## Part 9 — What's next

The single highest-value next move is finishing what we started this week.

After the Mac front door lights up, the Home dashboard shows **three of eight system services as ready** — the three host providers our bootstrap script installs. The other five (WebSpaces, Content Exchange, Site Edge, Public Edge, and the five "home" surface capsules) still show "missing prerequisites." Those services are optional (the Home won't refuse to start over them), but they unlock real apps: serving your website, sharing files publicly, running peer-to-peer content exchange.

**Tomorrow's goal is to extend the bootstrap script so all of them install in one shot.** When that lands, the Mac source checkout will go from "Home launches" to "Home has working apps." That's the moment ElastOS on Mac stops being a "kind of works" demo and becomes a real platform.

Beyond tomorrow: a notarized Mac installer (so users don't have to run a bootstrap script), a third-party security audit (so the sovereignty claim is verifiable), and the first non-trivial new app — most likely a sovereign personal AI that reads your notes and answers your questions, all locally.

---

## Closing

If a friend asks you what we're building, you can say:

> "We're building an operating system that gives you back your computer. The stuff Big Tech rents to you — your identity, your messages, your AI, your data — runs on hardware you already own, in sandboxes that can't leak, talking peer-to-peer to other people, with you holding the only keys. It already works on Linux, and as of this week, it launches on a Mac."

That's the whole thing.

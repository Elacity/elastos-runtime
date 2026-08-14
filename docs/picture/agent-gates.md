# Agent gates

Read this before you edit ElastOS. If a change fails a gate, stop.

## Before you write

Name the layer: Home, ESP, Runtime, Bus, provider, Carrier, or capsule.
Name the capability. Name what the capsule must not see.
Name the done-check a stranger could run.

## Refuse

Host filesystem paths in Home, People, or a capsule.
A capsule picking Carrier, IP, port, peer, ALPN, WebRTC, or WireGuard.
WireGuard as a Runtime or capsule contract.
A raw CEK in an app, Carrier, Runtime, or an ordinary receipt.
Seed treated as the product Home.
`POST /api/provider` as the new product ABI.
A mega-PR. One slice, one review branch.
A push, merge, or deploy Anders did not ask for.
A course change to Codex without reading its branch and history first.

## Hold

Capsules ask Runtime for a typed resource.
Runtime owns identity, capability, provider selection, coordinate, audit.
Providers own protocol meaning.
Carrier is the off-box pipe Runtime may select. Same-node delivery, when it exists, uses the same envelope and receipt.
dKMS talks only through Runtime-selected Carrier. That is the product rule even when `PRINCIPLES.md` §15 still allows "another substrate."
Fail closed, then explain.
Docs, code, and tests that disagree are a bug.

## After you write

The done-check passes.
`known-issues.md` is updated if you found a new one.
You did not teach a stale diagram as law.

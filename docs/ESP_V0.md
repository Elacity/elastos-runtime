# ESP v0 — the ElastOS Shell Protocol

> **Naming / branding.** **ESP = ElastOS Shell Protocol** — the read-only projection contract between
> the trusted Rust core and any shell (UI). ⚠️ Acronym collision: this is **unrelated** to IPsec's
> "Encapsulating Security Payload" (a networking/VPN term). With a technical audience, spell it out on
> first use — "ESP, our ElastOS Shell Protocol" — then abbreviate.

> **Status: v0, extracted from shipped state.** ESP is the read-only projection
> contract between the ElastOS runtime (the trusted core that *proves* and
> *gates*) and any shell that renders it. Every fact in ESP is a projection of
> real runtime state — there is no fact the core does not already emit. This doc
> is written by **extraction**: each fact/verb below cites the exact struct +
> route that produces it today. The shared TypeScript types are
> [`elastos/esp/esp_v0.ts`](../elastos/esp/esp_v0.ts) (type-checked with
> `tsc --noEmit --strict`).
>
> **Wedge W4.** Foundations: W2 (consent), W0 (reach), W1a (egress model), W3a
> (de-hardcoded shell). A v0 projection shell (W5) consumes this contract.

## 1. The inviolable rule

A shell is a **read-only projection**. It renders facts and *requests* verbs; it
never holds authority. Enforcement (capability gate, consent, signed audit) stays
in the core. A shell can be written in any language against these JSON shapes; it
cannot bypass the gate because it never has a key — it only sends verb requests
the core independently validates.

## 2. Transport (what the code actually serves today)

- **Facts** travel over plain **HTTP GET → JSON** on the routes below.
- The **consent flow** is HTTP: `POST …/invoke` → **HTTP 202** + a
  `affordance-consent-pending` fact; approval via the capability verbs; redemption
  via `validate-and-consume`.
- ESP is **transport-independent by contract**: a fact is identified by its
  `schema` tag, not its route, so the same facts may later travel over a stream
  or a different edge without changing their meaning.

> **Honest note (no over-promise):** there is **no push/SSE stream and no
> initialize handshake implemented today** — a shell reads the projection routes
> directly. §6 *defines* an initialize handshake as the forward contract, marked
> **not-yet-implemented**.

## 3. Versioning & extensibility

- **Per-fact versioning.** Every fact carries a `schema` tag `elastos.<family>/vN`
  (e.g. `elastos.capsules.catalog/v1`). A consumer keys off the tag, never the
  route shape. A breaking change bumps the tag.
- **Must-ignore-unknown (FACTS).** A shell **MUST ignore unknown fields** on any
  fact it reads, so the core can add fields without breaking shells. The TS types
  model this with an index signature on fact types.
- **Caveat — verb INPUTS are strict.** The runtime's *verb request bodies*
  (`RequestCapabilityInput`, `ValidateAndConsumeInput`,
  `CapsuleInterfaceInvokeRequest`) currently use `#[serde(deny_unknown_fields)]`
  and **reject** unknown keys. Must-ignore-unknown is a rule for facts the shell
  **reads**, not for bodies it **sends**. A future slice may relax verb inputs for
  forward-compat; until then, send exactly the documented fields.

## 4. The projection fact families

| # | Family | `schema` tag | Route | Source struct (file) |
|---|---|---|---|---|
| 1 | Capsule catalog | `elastos.capsules.catalog/v1` | `GET /api/capsules/catalog` | `CapsuleCatalogResponse` / `CapsuleSummary` (`gateway_capsule_catalog.rs`) |
| 2 | Interface registry | `elastos.capsules.interfaces/v1` | `GET /api/capsules/interfaces` | `CapsuleInterfaceRegistryResponse` (`gateway_capsule_catalog.rs`) |
| 3 | Affordance reach | `elastos.reach.v1` (embedded) | embedded in family 1 (`affordance_reach`) | `AffordanceReachView` + `ReachDescriptorV1` (`gateway_capsule_catalog.rs`, `elastos-common/src/reach.rs`) |
| 4 | Consent pending | `elastos.capsules.affordance-consent-pending/v1` | `202` from `POST /api/capsules/interfaces/invoke` | `AffordanceConsentPending` (`gateway_capsule_catalog.rs`) |
| 5 | Grant receipt | `elastos.affordance.receipt.v1` | returned by `validate-and-consume` | `AffordanceGrantReceiptV1` (`elastos-runtime/src/capability/receipt.rs`) |

### 4.1 Affordance reach — the blast-radius halo (W0/W1)

`ReachDescriptorV1` is **core-computed** from the capsule's *enforced* capability
(isolation tier + net permission) and the affordance's concrete `(resource,
operation)` — **never** the self-declared `risk`. Dimensions: `egress`
(`none|allowlisted|open`), `isolation` (`data|wasm|micro_vm|host_process`), `scope`
(`object|collection|system|unknown`), `reversibility`
(`reversible|one_way|unknown`), and `observed`. `AffordanceReachView` adds
`declared_understates_reach` — the **"a clone must lie"** flag (claims low, reaches
far).

> **DEGRADED in v0:** `egress: "allowlisted"` is **modeled, not yet enforced** at
> the network boundary (W1b — needs KVM/`CAP_NET_ADMIN`); today the core emits
> `none`/`open`. When `observed` is `false`, a dimension could not be pinned —
> render the halo **incomplete**, never falsely cool.

## 5. The verbs

| Verb | Route | Auth | Result | Source |
|---|---|---|---|---|
| Request consent / capability | `POST /api/capability/request` | session | `pending` + `request_id` (or `granted`/`denied`) | `request_capability` |
| Approve | `POST /api/capability/grant` | **shell-only** | grant; token retrievable via `GET /api/capability/request/:id` | `grant_request` |
| Deny | `POST /api/capability/deny` | **shell-only** | denied (fail-closed) | `deny_request` |
| Validate-and-consume (redeem) | `POST /api/capability/validate-and-consume` | session | `consumed` + signed `AffordanceGrantReceiptV1` | `validate_and_consume` |
| Invoke affordance | `POST /api/capsules/interfaces/invoke` | home-launch token | `202` consent-pending (1st call) → redeem+dispatch (retry w/ `consent_token`) | `capsule_interface_invoke` |
| Issue standing grant | `POST /api/standing-grants/issue` | **shell-only** | mints a real signed token → derives + stores the standing envelope → `{grant_id}` (400 on bad action / empty methods) | `issue_standing_grant` |
| Revoke standing grant | `POST /api/standing-grants/revoke` | **shell-only** | `{revoked}` — the autonomy kill switch (fail-closed) | `revoke_standing_grant` |
| Preview standing-grant intent | `POST /api/standing-grants/preview` | **shell-only** | dry-run: verify a signed `IntentDeclarationV1` then `{verdict, reason}` — records nothing, runs no act (400 on bad signature) | `preview_standing_grant` |

### 5.1 Standing grants — unsupervised-agent authority (Tier 2c)

A standing grant authorizes an agent to declare-and-act **repeatedly** on a
`(resource, action)` for a set of methods, without a human in the loop per act —
the net-new dispatch mode of the intent-proof loop
([`INTENT_PROOF_LOOP.md`](INTENT_PROOF_LOOP.md)). The three verbs above are
**shell-only** (behind the same `consent_broker_only_middleware` as approve/deny —
issuing authority is privileged; an ordinary capsule session can never reach them)
and **fail-closed** (a missing grant, a revoked/expired envelope, an out-of-envelope
method, or a signature that does not verify all deny). `issue` roots every grant in
a **real signed capability token**; `revoke` re-reads on each dispatch, so it denies
every not-yet-started act; `preview` is a side-effect-free dry-run for dashboards.
The signed declared-vs-done reconciliation each dispatch writes is what the inspector
paints as the **intent custody channel** (Tier 2b).

> **Honest scope:** the side-effecting **dispatch/act route** (an agent actually
> invoking an affordance over HTTP under its grant) is **not yet served** — it needs
> the affordance-invocation wiring decision. In-process dispatch (`dispatch_standing_act`
> / `StandingGrantService::dispatch`) is built, gated, and available to the runtime today.

**The consent journey (W2):** invoke (consent-gated) → `202` consent-pending →
approve → retry invoke with the granted token → `validate-and-consume` re-checks
the exact `(method, args)` and atomically spends the single use → signed receipt.
Every mismatch fails closed.

> **DEGRADED in v0:** the live gateway→runtime redeem round-trip (forwarded
> caller bearer authenticating as `vm-{name}`) is **integration-verified, not
> unit-tested**; the redeem→dispatch path lands fully with the hero act (W5).

## 6. Initialize handshake (DEFINED — not yet implemented)

A shell SHOULD open an ESP session by declaring the schema tags it understands, so
the core can negotiate fact versions:

```jsonc
// POST /api/esp/initialize   (NOT YET SERVED — forward contract only)
{ "esp_version": "0", "accepts": ["elastos.capsules.catalog/v1", "elastos.reach.v1", ...] }
```

Until served, a shell reads the projection routes in §4 directly and keys off each
fact's `schema` tag. This section is the contract a later slice implements.

## 7. Conformance & drift

- **Types:** [`elastos/esp/esp_v0.ts`](../elastos/esp/esp_v0.ts) mirrors the serde
  shapes (enum values are the `rename_all = "snake_case"` forms); type-checked
  with `tsc --noEmit --strict`.
- **Anti-drift:** `scripts/check-wci-alignment.sh` pins that the routes and
  `schema` tags this doc documents still exist in the code, so ESP_V0.md cannot
  silently diverge from what the runtime serves.
- **Sync:** the TS types are hand-maintained against the Rust structs today; a
  future slice may codegen them from the serde definitions.

## 8. What v0 deliberately does NOT claim

- No push/SSE stream and no initialize endpoint are implemented yet (§2, §6).
- `egress: "allowlisted"` is modeled, not enforced (W1b).
- The live redeem→dispatch round-trip is integration-verified, not unit-tested (W5).
- Verb input bodies reject unknown fields today (§3).

# Threat model

This page is the first read for a security researcher. It describes the ElastOS
0.8.0 tree. Every statement below was checked against that tree. There has been
no external audit. `docs/audits/` holds internal review notes only: the
workbook in `docs/audits/README.md` is a historical snapshot, and the other
files there are dated internal notes.

## Who mints and approves capability tokens

The Runtime mints grants and checks them on use. `docs/ARCHITECTURE.md` states
that Runtime alone mints grants. `CapabilityManager::grant` builds the token
and signs it with the Runtime key
(`elastos/crates/elastos-runtime/src/capability/manager.rs`). The shell-only
route `POST /api/capability/grant` calls that function
(`elastos/crates/elastos-server/src/api/handlers/capability.rs`). On use,
`CapabilityManager::validate` checks version, signature, issuer, caller,
action, resource, epoch, individual revocation, time, classification, and use
count.

In this release the shell approves pending requests. It does not ask the
person each time.

Managed Home and Chat start the Runtime with `ELASTOS_SHELL_MODE=agent` and a
policy file (`ensure_managed_runtime` in
`elastos/crates/elastos-server/src/runtime_control.rs`). A hand-run
`elastos serve` does the same unless `ELASTOS_SHELL_MODE` is set
(`shell_capsule_command` in `elastos/crates/elastos-server/src/serve_cmd.rs`).
`resolve_mode` in `elastos/capsules/shell/src/main.rs` selects agent mode for
that setting, and whenever stdin is not a terminal. Agent mode loads
`PolicyRulesEngine`. A request is granted for the session when its resource
matches an allow rule. A rule that ends in `*` is a prefix match. Any other
rule must equal the resource. The action is not part of the match. A missing,
unreadable, or malformed policy file allows nothing.

The managed Home allow-list (`managed_home_allow_resources`) is
`elastos://peer/*`, `elastos://did/*`, `localhost://Users/*`, and
`localhost://Local/SharedByLocalUsersAndBots/Home/*`. The managed Chat
allow-list (`managed_chat_allow_resources`) is the first three of those.

Full auto-grant mode runs only when `ELASTOS_SHELL_MODE=auto`. `resolve_mode`
returns `auto` only for that explicit value. `AutoGrantEngine` then grants
every request, and the shell prints that the mode is for development.

Per-app approval on a screen that only the Runtime controls is planned for
0.8.1. It is not in this release (`elastos/CHANGELOG.md`, the 0.8.0 "not done
yet" note). `cli` mode can prompt on a terminal (`g`, `o`, `d`, `s` in
`CliPromptEngine`). Managed Home and Chat do not select it.

The grant handler stores the request's session id as the token's capsule id.
A session grant sets no expiry. A "once" grant sets a use limit of one.

## Assets

- Files, documents, media, and messages the person keeps in Home.
- Passkeys, profile and device keys, wallet keys, and the Recovery Kit. Keys
  are stored next to the data they protect (`SECURITY.md`, Stored keys and
  recovery).
- Hosted model API keys. The Runtime writes each key in an owner-only
  directory and attaches it to the outbound request
  (`write_hosted_secret` in
  `elastos/crates/elastos-server/src/api/model_provider_config.rs`;
  `elastos/crates/elastos-server/src/api/model_provider_egress.rs`).
- The capability signing key, `{data_dir}/signing_key`, and the capability
  store, `{data_dir}/capability_store`
  (`CapabilityManager::load_or_generate`; `serve_capability_store` in
  `elastos/crates/elastos-server/src/server_infra.rs`).
- Installed Runtime, provider, and capsule bytes named by a signed release.

## Actors

| Actor | Role in this release |
| --- | --- |
| Runtime | Mints and checks capability tokens. Holds the signing key. |
| Person who owns the Home | Passkeys control sign-in. Home and Chat grants follow the allow-list above. |
| Apps and agents | They submit requests. They do not sign grants. |
| Model output | It can propose an effect. It does not authorize one (`docs/ARCHITECTURE.md`, Provider capsule interface; `docs/MODEL_PROVIDER.md`). |
| Home | Home can obtain every app's capability, so a compromised Home can reach those apps' authority (`SECURITY.md`; `docs/ARCHITECTURE.md`). |
| Shell | Decides pending grants from the policy file, or from the development auto-grant switch. |
| Providers | Native processes with the Runtime user's rights. Only the model provider is partly confined. See [known limitations](KNOWN-LIMITATIONS.md). |
| Other devices | Carrier authenticates the transport peer. It does not check application-message signatures. |
| Release publisher | A Home trusts release envelopes signed by a publisher DID it already stores. |

## Trust boundaries

1. **App or agent to Runtime.** A requested effect becomes a pending capability
   request. The shell answers it. The Runtime mints the token and checks it
   on use.
2. **Capsule bytes to this Home.** `verify_capsule_signature` in
   `elastos/crates/elastos-server/src/runtime.rs` checks a signed capsule
   against configured trusted keys, over a hash of the entrypoint file. An
   unsigned capsule runs only when development mode allows unsigned capsules,
   or when `verify_installed_release_capsule` in
   `elastos/crates/elastos-server/src/setup.rs` matches the name, install
   path, and CID to this Home's publisher-signed `components.json`.
3. **Runtime to a hosted model service.** The Runtime keeps the API key and
   sends the HTTPS request. The model reply is input, not a grant.
4. **This device to other devices.** Carrier is endpoint-authenticated
   transport (`docs/CARRIER.md`). Gossip `sender_id` and `signature` are
   caller-controlled, and Carrier does not validate them
   (`elastos/crates/elastos-server/src/carrier.rs`, trust-boundary comment).
   Native chat checks a received payload through the DID provider
   (`verify_via_did_provider` in
   `elastos/crates/elastos-server/src/chat_cmd.rs`).
5. **Installer and updates to this device.** Update discovery verifies the
   release envelope against a trusted publisher DID before it accepts the head
   (`verify_release_envelope` in `elastos/crates/elastos-server/src/update.rs`
   and `elastos/crates/elastos-server/src/crypto.rs`). `README.md` publishes
   the maintainer DID the public installer prints.

A gateway request whose Origin authority differs from Host is rejected
(`effective_gateway_origin` in
`elastos/crates/elastos-server/src/api/gateway_origin.rs`). The Runtime control
API accepts only its own Origin (`runtime_api_origin_allowed` in
`elastos/crates/elastos-server/src/api/server.rs`).

## Attackers this page covers

- A capsule that requests a resource outside the allow-list, or that presents
  another caller's token.
- A model reply that tries to authorize an effect by itself.
- A capsule tree that fails the signature or signed-release CID checks above.
- A release envelope signed by a DID this Home does not trust.
- A gateway request whose Origin names a different host from the Host header.

A remote peer can send a large Carrier or I/O-bridge frame. Size caps exist,
and two read-time gaps remain open. See `SECURITY.md`.

## What these checks do

The shell allow-list allows nothing when the policy file is missing or
broken. Tokens carry the Runtime signature. The checker honors expiry and a
use limit when the token sets them. Revocation is checked against the
capability store loaded for that process. WASM component capsules run inside
the Runtime process, with a 128 MiB memory limit and a fuel budget of
100,000,000 (`elastos/crates/elastos-compute/src/providers/component.rs`).
They share the operating-system process with the Runtime keys (`SECURITY.md`).

## Out of scope for this release

Per-app user approval is planned, not shipped. Another program in the same OS
account, root, or the operator of a hosted seed can read the data directory,
including keys, while Home is locked. Passkeys control sign-in
(`SECURITY.md`). After the Runtime sends a hosted-model request, that
service's own authorization is outside this tree. The shell allow-list does
not ask the person before it grants a matching resource, including a request
that followed a model reply.

Operating-system confinement, the Linux Browser gap, and the open findings are
in [known limitations](KNOWN-LIMITATIONS.md).

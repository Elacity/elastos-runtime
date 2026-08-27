# Documentation map

Each top-level ledger has one job:

- [state.md](../state.md): current verified behavior and known gaps
- [TASKS.md](../TASKS.md): open work
- [ROADMAP.md](../ROADMAP.md): future direction
- [elastos/CHANGELOG.md](../elastos/CHANGELOG.md): released history
- [PRINCIPLES.md](../PRINCIPLES.md): decision constraints

## Start here

- [Repository README](../README.md): quick install, source build, and system model
- [Getting started](GETTING_STARTED.md): user installation and source development
- [Local source Home setup](HOME_LOCAL_SETUP.md): source-home browser Home on one machine
- [Installing ElastOS](INSTALL.md): Linux setup, update, and trust
- [Glossary](GLOSSARY.md): canonical terminology

## Architecture and core model

- [Architecture](ARCHITECTURE.md): responsibility, authority, and isolation boundaries
- [Capsule model](CAPSULE_MODEL.md): artifact, Runtime contract, instance, state,
  and head
- [Namespaces](NAMESPACES.md): `localhost://`, `elastos://`, and principal roots
- [Carrier](CARRIER.md): endpoint-authenticated communication and content transport
- [Content availability](CONTENT_AVAILABILITY.md): CID, IPLD, availability, and replication
- [People and conversations](PEOPLE_CONVERSATIONS.md): profiles, contacts,
  discovery, and current Chat integration
- [Design system](DESIGN_SYSTEM.md): first-party visual and interaction contract

## Runtime and interface contracts

- [Principles](../PRINCIPLES.md): normative authority constraints
- [ESP v0](ESP_V0.md): shell-facing Runtime facts and intents
- [Shell/ESP boundary map](SHELL_ESP_BOUNDARY_MAP.md): placement rules for
  Runtime, providers, shells, and shared projection code
- [Capsule interface contract](CAPSULE_INTERFACE_CONTRACT.md): web, CLI, fact,
  gate, and audit projections
- [Home shell host contract](HOME_SHELL_HOST_CONTRACT.md): sign-in, shell
  lifecycle, and child intents
- [Interactive runtime contract](INTERACTIVE_RUNTIME_CONTRACT.md): interactive
  sessions and return behavior
- [Command runtime matrix](COMMAND_MATRIX.md): Runtime ownership for every
  command
- [Authentication audit chain](AUTH_AUDIT_CHAIN.md): activation and retention
  rules for the signed audit chain
- [Capsule Inspector](CAPSULE_INSPECTOR.md): live facts and gate preview; Inbox
  owns approval and Runtime owns dispatch
- [Capsule authoring](CAPSULE_AUTHORING.md): supported roles, ABI, Bus, manifests, and verification
- [Capsule templates](../templates/capsules/README.md): checked-in starting points
- [ElastOS Bus WIT](../elastos/wit/elastos-bus-v1.wit): executable Component interface

## Provider and content contracts

- [Peer resource contract](../elastos/docs/PEER_PROTOCOL.md): peer bootstrap,
  topic membership, gossip operations, and trust limits
- [Chain provider](CHAIN_PROVIDER.md): typed chain reads, proofs, and transactions
- [Wallet provider](WALLET_PROVIDER.md): account, proof, approval, and signing authority
- [Protected content](PROTECTED_CONTENT.md): sealed object access sequence
- [Protected-content v1 contracts](PROTECTED_CONTENT_CONTRACTS_V1.md): canonical
  source-only review candidate
- [Rights provider](RIGHTS_PROVIDER.md): canonical role and provisional capsule
  retirement state
- [Key provider](KEY_PROVIDER.md): provisional provider retirement notice
- [Decrypt provider](DECRYPT_PROVIDER.md): canonical role and provisional
  capsule retirement state
- [Archive policy](ARCHIVE_POLICY.md): archive dependencies and family enablement

## Browser contracts and decisions

- [Browser capsule](BROWSER_CAPSULE.md): Browser, Net, Exit, and Engine contract
- [Browser VM target](BROWSER_VM_TARGET.md): VM guest, helper, media, and target maintenance
- [Browser provider acceptance](BROWSER_PROVIDER_BAKEOFF.md): shared candidate
  gates; current status comes from generated evidence

## Operations and verification

- [0.6.0 release acceptance](RUNTIME_REPO_USER_STORY_CHECKLIST.md): source,
  installed-product, and release decision checklist
- [Mac source-home staging](MAC.md): Apple silicon staging and Browser acceptance
- [Inspector testing](INSPECTOR_TESTING.md): local Inspector checks
- [Sites](SITES.md): local site roots and public exposure
- [Scripts](../scripts/README.md): build, proof, release, and operator commands
- [Debugging](../DEBUG.md): developer diagnostics
- [Security](../SECURITY.md): reporting policy and verified security findings

## Versioning

- [Runtime versioning](VERSIONING.md): release version policy
- [Share versioning](SHARE_VERSIONING.md): immutable revisions and mutable heads

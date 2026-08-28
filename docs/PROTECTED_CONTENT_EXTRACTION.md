# Protected-content extraction ledger

This ledger records how the current Runtime-owned protected-content source was
assembled and what installed evidence remains. The current tree is the source
of truth. Historical branches and PRs are evidence only.

## Current source identity

| Source | Commit | Role |
|---|---|---|
| `origin/feat/protected-content-runtime-lifecycle` | `854d9dc945b6ecd53731af7edb382847d92cbb76` | Published inactive lifecycle source. |
| `origin/feat/0.7-uiux-candidate` | `8b547590335e25126aca726135976d415433cea6` | Published reviewed UIUX donor. |
| Unpublished implementation prefix | `3ece9042df1193ec6873145971557103fdd4f45a` | Reconstructed source through cross-Runtime release admission and playback envelope removal; tree `a17c799d4ffd837a4c65888ec9601ee6216c52fa`. |

The integrated prefix remains unpublished source evidence. It is not installed
or live product proof. The active installed path still selects the provisional
`drm`, `rights`, `key`, and `decrypt` surfaces.

The current inactive proof runs on one Runtime with two principals. It does not
prove mint on localhost followed by buy and play on the seed.

## Extraction ownership

Runtime owns authenticated authority, provider selection, durable operation
identity, lifecycle, Wallet and Chain coordination, audit, and settlement.
Providers own protection, media preparation, custody, decryption, content,
availability, Wallet, Chain, and model operation semantics. Carrier transports
Runtime-selected remote endpoint traffic. Capsules own app behavior and
presentation.

The integrated source has one typed protected-content path:

`Library -> Runtime -> media -> protect -> content/availability -> Wallet/Chain -> custody -> protected-content-decrypt -> elacity-player`

The path keeps KID and `EncryptedContentIdentityV1` separate. It binds mint,
listing, purchase, availability, rights, release, viewer launch, media reads,
and close to the exact principal and protected object. Runtime records
identities and receipts while private providers retain media, key, share,
process, and route material.

## PR15 and dKMS evidence

PR #15 / `feat/dkms-esp-port` is source evidence, not a merge target. Its
useful product and test ideas were adapted to current authority boundaries:

- `6d2e9083` supplied player/viewer behavior and Library-open UX. The current
  source uses typed Runtime launch, read, and close operations.
- `c5aed9db` supplied Creator UX. The current Library capsule owns the
  protect-and-list flow.
- `57974479` supplied a grant journey. Current source binds that journey to
  Profile identity, the authenticated session, verified Wallet authority, and
  Runtime operation identity.
- `ffea5998` supplied useful Create, mint, and open failure cases. Current
  typed Library, Runtime, provider, and viewer paths cover the applicable
  cases.
- `e148218b` supplied CI lessons. The current focused source, platform,
  inventory, entropy, and installation gates retain the applicable lessons.

Current video presentation uses `elacity-player`. Document and 3D viewer work
remains later typed-viewer scope.

The extraction retained threshold reconstruction, independent node custody,
recipient-sealed contributions, CEK commitment checks, bounded lifecycle
scenarios, and fail-closed negative cases. Current Runtime and provider
contracts own those behaviors. Historical capsule-owned authority, public
aggregated share metadata, raw CEK APIs, direct topology, old DRM
orchestration, and standalone harness routes are not current product paths.

## Deployed contract truth

Verified Base read behavior is:

- `AuthorityGateway.hasAccessByContentId(address,bytes16) -> bool` owns the
  access read.
- The bytes16 argument is the CENC KID. It remains separate from the full
  `EncryptedContentIdentityV1`.
- `CentralStorage.ipReference(bytes16)` is the proven KID read resolution.
- Unknown KIDs revert with `UnboundContentId(bytes16)`; a bound KID without
  access returns `false`.

Verified Base 8453 evidence from `origin/upstream/0.7-dev@90bbe15b` records:

- `CentralStorage.bindIP(bytes16,address,uint256)` is acknowledged-contracts
  only and is called by `AssetFactory.registerNewAsset`;
- native `AuthorityGateway.buyAccess` selector `0xf7580ad9`;
- ERC20 `AuthorityGateway.buyAccess` selector `0x0ede2294`, with prior approval
  to each operative `paymentProcessor()`;
- EventHub as the mint emitter; and
- bound-KID allowed, denied, and unbound results.

The exact funded transaction receipt and event remain installed proof. Chain
access is one boolean per holder and KID. Signed Runtime policy owns the View
and Download action distinction.

Each custody node reads rights through its node-host Chain provider. The
owner-only protected-content Chain config supplies 2-5 explicit private RPC
sources. Rights evidence requires two exact agreeing finalized results and has
no generic-network fallback. Provider, RPC, storage, and Carrier topology
remains private.

## Source proof

The current source covers:

- bounded media preparation and private staging;
- CENC protection and immutable custody-envelope creation;
- exact three-replica publication and repair-task persistence;
- fresh signed availability before purchase and open;
- Runtime-owned creator listing and buyer Wallet-account binding;
- exact transaction-effect replay and finalized evidence;
- private 2-of-3 custody release;
- decrypt-provider reconstruction and bounded ordered viewer reads;
- Library creation and playback;
- Marketplace listing, buy, and player launch;
- restart, replay, tamper, wrong-identity, timeout, cancel, and cleanup cases;
- private Runtime-only provider registration and readiness settlement; and
- stable source-home installation, static installed audit, and exact macOS and
  Linux restart helpers.

This proof uses focused source and fixture tests. It does not establish real
operator custody, deployed Chain acceptance, live replication, or an active
cutover. Custody release now authenticates the buyer Runtime issuer declared by
the signed operation and bound by the buyer Profile, while provisioning stays
pinned to the creator Runtime. A buyer Runtime still needs typed import and
verified projection for the creator's immutable listing, which remains a local
Runtime record bound to its content and Chain identity.

A shared listing link is sufficient for 0.7. Global listing discovery and
public custody governance remain later work. The public protected-content
contracts stay frozen for this source slice.

## Installed artifacts

Full source-home setup installs these canonical private native providers once:

- `protected-content-protect-provider`
- `media-provider`
- `custody-provider`
- `protected-content-decrypt-provider`

Their component declarations use reserved Runtime-only targets. Public capsule
inventories and public provider interfaces exclude them. The provisional
providers remain packaged until cutover so the inactive source can be installed
without changing the active authority path.

Full setup also installs one stable Runtime at `bin/elastos` under the
platform data root and writes
`receipts/source-home-installation.json`. The versioned receipt binds the
current source commit, tree, clean state, built and installed Runtime hashes,
installed components hash, capsule metadata receipt hash, platform, and
installation time.

`scripts/protected-content-installed-static-audit.py` reads the source and
installed trees. It verifies artifact parity, private provider declarations,
stable locations, profile and role prerequisites, Kubo for Home, owner-only
protected-content configuration, media tools, and custody prerequisites. Its
redacted receipt distinguishes:

- `source_static_artifact_failures`;
- `operator_configuration_prerequisites`; and
- `active_installed_proof_prerequisites`.

A successful static audit reports `ready_for_active_proof`. Active proof still
owns Runtime acceptance, signed custody validation, live Chain evidence,
provider startup, replication and repair, and mint-buy-play.

## Platform restart truth

Both platform restart helpers validate the stable installation receipt and the
current clean source identity before process or migration effects. They use one
owner-only PID file, bind the prior process to its recorded Runtime hash and
exact command identity, and publish the new PID only after the new stable
Runtime owns the listener. One bounded principal-root rollback is retained for
operator reconciliation.

The macOS fixture proves replacement restart on this host, including a live
prior Runtime whose installed binary was atomically replaced. Linux source and
fixture checks cover the `/proc` identity model. Active Linux
`/proc/<pid>/exe`, listener, and replacement behavior remains target evidence.

## Remaining installed proof

Complete the sequence in [TASKS.md](../TASKS.md):

1. repair the stale private-custody gateway fixture and restore the combined
   one-Runtime proof;
2. add portable listing export, import, and projection;
3. prove the full two-Runtime source journey;
4. review the integrated source and prepare the review branch;
5. install the exact same tree on localhost, the seed, and the third custody
   node with matching stable
   receipts;
6. supply a real signed owner-only 2-of-3 custody composition across distinct
   operators;
7. supply the private multi-source Chain config and prove deployed Base reads;
8. prove one bound KID, allowed and denied Wallet accounts, CentralStorage
   binding, and the exact `AuthorityGateway.buyAccess` effect;
9. prove three replicas and repair after one loss;
10. run the two-Runtime, two-principal mint-list-deny-buy-open-play-close path
   with restart, replay, tamper, settlement, and cleanup evidence;
11. complete the named manual UIUX matrix in `TASKS.md`; and
12. make one atomic cutover that removes every provisional authority surface.

The atomic cutover updates startup, registration, resources, packaging, tests,
and docs together. It leaves one Runtime coordinator, one ProviderRegistry,
one provider lifecycle, and one protected-content path.

External cryptographic review remains open. Global listing discovery, public
custody governance, and broader viewer types remain separate later projects.

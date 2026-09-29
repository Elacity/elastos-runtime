# J3 bootstrap intake

The delivery branch is published at `00003b6f`; Anders chose continued work on
`feat/0.7.1-website-execution`. Notion's source, installed checkpoint and next-step
records were updated and read back on 2026-09-11. D1 to D6 remain recorded and
all journey acceptance remains intact. Recurring monitoring stays paused.

## Installed starting point

The Mac ARM64 human Home still runs `1e320578`. Its Runtime and the inspected
Home Agent/Assistant assets match their existing installation receipts. Home
Agent shows zero model offers. All models opens empty Settings with older Store
guidance, and the Apps launcher lists both Assistant and Home Agent. This read-only
inspection started no model request or transfer and preserved the user's state.
Linux was not newly exercised during this slice.

This is an earlier setup gap than the integration donor's historical failure
after metadata progress. The cause of that older attempt remains unknown.

## Source result

Donor `c95cf4c9` supplies the engine/model bootstrap prerequisite. Runtime and
the fetch helper share an exact engine bundle inventory and verify the pinned
artifact, files, links, executable and ownership before reuse. The manifest pins
the engine and model inputs. Actual model delivery and invocation remain pending.

The donor renamed the setup cache helper. The first compile exposed three
current media call sites using the old name; all three now pass the platform to
the new helper. The existing media reuse and integrity behavior is preserved.

| Required behavior | Source | Target and check | Result |
| --- | --- | --- | --- |
| Verify platform-selected engine/model inputs and reuse valid bytes | `components.json`, `scripts/fetch/fetch-model.sh` | Mac-hosted synthetic platform fixtures: `python3 scripts/fetch/fetch-model-smoke.py` | Pass |
| Reject altered, missing, added or unsafe bundle entries and invalid protection | Fetch helper and engine receipt | Same isolated bootstrap smoke | Pass |
| Preserve existing bytes after checksum failure; clean interrupted download | Fetch helper | Same bootstrap smoke with small loopback fixtures | Pass |
| Runtime setup and fetch agree on bundle identity and stable executable path | `setup.rs`, `setup/local_model_engine_receipt.rs` | Mac ARM64: `cargo +1.91.0 test --locked -p elastos-server setup::tests:: -- --test-threads=1` from `elastos/` | 56 pass, including media reuse/integrity tests |
| Preserve basic repository contracts | Current source | Diff check, Home entropy, workspace and chain-provider formatting | Pass |

One owner ran source edits and tests. Independent review found the same helper
integration issue and no further blocker for this standalone bootstrap slice.
The tests used temporary test-owned data. No model weights were downloaded, and
no Runtime/provider was installed or restarted. Linux installed acceptance keeps
its prior identity and verdict. Private diagnostics and artifact identities are
in the operator checkpoint.

## Next executable slice

Integrate the corrected local provider and catalog/admission core from donor
`6972e165`, including cancellation, capacity, retirement ownership and bounded
failure diagnostics. Preserve current auth/recovery, scoped cookies, routes,
shutdown and media behavior. J1 conditional-save and window policy changes are
independent of this first model diagnostic and retain their own acceptance.

Before one test-owned Home preparation attempt, verify signed catalog trust and
expiry, grant, disk admission, artifact identity and normal cancellation controls.
Capture the first terminal failure, or cancel at the agreed observation limit,
then prove settlement. Reuse valid existing inputs. Marketplace handoff,
retention/removal, assistant consolidation and Required cold Content/Carrier
delivery remain subsequent J3 work; donor presence does not close these criteria.

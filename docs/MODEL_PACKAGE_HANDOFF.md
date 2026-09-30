# Model package handoff

This document tells a sender operator and a receiver how to move one signed
model content package from the sender's Runtime-managed Kubo into the
receiver's Runtime-managed Kubo, and how the receiver pins the sender's signed
permanent catalog snapshot so their Runtime can run normal local Use. The
helper is `scripts/model-package-handoff.py` and needs only Python 3. Catalog
and package rules live in
[CONTENT_CAPSULE_DISTRIBUTION.md](CONTENT_CAPSULE_DISTRIBUTION.md).

## Purpose and boundaries

The package travels as a CAR file. A CAR is a portable copy of the exact block
closure under the package root CID, so the receiver's Kubo ends up with the
same blocks and the same root the sender holds. CAR transfer is package
portability between two operators. Cold Content or Carrier Get for models
remains a separate, unaccepted flow; this handoff does not stand in for it.

Two pins exist and stay separate. The Content retention pin is a Kubo recursive
pin of the package root CID inside the receiver's Kubo; it keeps the bytes. The
catalog trust pin is `components.json.model_catalog.head_cid`, the raw CIDv1
SHA-256 of the exact bytes of the signed `model-catalog.json` snapshot; it tells
the Runtime which publisher statement to trust. The helper writes each pin with
its own subcommand, removing one leaves the other untouched, and neither pin
implies the other.

The Runtime verifies the catalog signature when it reads the snapshot. The
helper checks the head CID, the schema, the entry, the signer DID and the
permanent-snapshot rule. It does not verify the signature, and its receipt
says `"signature_verification": "pending_runtime_read"` until the Runtime has
read and verified the snapshot. A permanent snapshot omits `expires_at`.
Runtime readers older than the permanent-snapshot change require that field
and reject a permanent snapshot, so the receiver's Runtime includes that
change before the receiver pins one.

Local Use also needs the platform llama.cpp engine. The Runtime derives
dispatch readiness from the current signed entry, the admission, the verified
`llama-server` engine receipt and one matching offer from the local model
provider. Current source has startup profiles for `darwin-arm64` and
`linux-amd64`; the latter uses a bounded CPU path. Linux readiness still needs
a compatible installed engine receipt and target proof. Other hosts stay not
ready until they have a verified profile.

The helper streams every transfer in 1 MiB chunks and hashes as it goes, so a
6 GB package stays on disk and travels as raw bytes end to end.

## Sender steps

The sender holds the package in their Runtime's Kubo. The example package root
is `bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi` with a payload
of 6169366387 bytes; the CAR is a little larger than the payload because it
carries block headers and the DAG-PB directory nodes.

1. Build the permanent catalog payload. It carries the schema, `published_at`,
   and one to eight entries. Each entry `cid` is a package root, its
   `capsule_manifest` is that package's `capsule.json` object, and its
   `object_manifest` is its `_elastos_object.json` object. Package CIDs and
   capsule names stay unique. Leave `expires_at` out. Serialize it with sorted
   keys and no whitespace, which is what the Runtime signs over. The example
   below shows one entry:

   ```bash
   PAYLOAD=$(jq -cS -n \
     --arg cid bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi \
     --argjson capsule "$(cat package/capsule.json)" \
     --argjson object "$(cat package/_elastos_object.json)" \
     '{schema: "elastos.model.catalog/v1",
       published_at: (now | floor),
       entries: [{cid: $cid, capsule_manifest: $capsule,
                  object_manifest: $object}]}')
   ```

2. Sign the payload with the publisher key and assemble the envelope. The
   Runtime accepts the envelope keys `payload`, `signature` and `signer_did`
   and nothing else:

   ```bash
   SIGNED=$(printf '%s' "$PAYLOAD" | \
     elastos sign-payload --domain elastos.model.catalog.v1 --key "$KEY")
   jq -cS -n --argjson payload "$PAYLOAD" \
     --arg signature "$(jq -r .signature <<<"$SIGNED")" \
     --arg signer_did "$(jq -r .signer_did <<<"$SIGNED")" \
     '{payload: $payload, signature: $signature, signer_did: $signer_did}' \
     > model-catalog.json
   ```

3. Compute the head CID of the exact file bytes. The receiver pins this value,
   so record it as `HEAD_CID` and hand it over as its own item:

   ```bash
   python3 scripts/model-package-handoff.py head-cid \
     --catalog model-catalog.json
   ```

4. Export the CAR from the Runtime's Kubo. The helper reads the Kubo API port
   from `ipfs-coords.json` in the Runtime data dir, checks that Kubo is 0.40.1
   with the canonical import profile, streams the closure to
   `qwen-package.car.partial`, renames it on success and writes the receipt:

   ```bash
   python3 scripts/model-package-handoff.py export \
     --data-dir "${XDG_DATA_HOME:-$HOME/.local/share}/elastos" \
     --cid bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi \
     --output qwen-package.car
   ```

   The receipt `qwen-package.car.receipt.json` records the package CID, the
   CAR SHA-256, the CAR size, the Kubo version and the export time.

5. Hand over five things: `qwen-package.car`, `qwen-package.car.receipt.json`,
   `model-catalog.json`, the exact `HEAD_CID` from step 3, and the publisher
   DID printed by `sign-payload`. Send the head CID and the publisher DID in
   text the receiver can compare against, not only inside the files. Any
   durable file transport works, for example a USB drive, rsync or an object
   store. The CAR is about 6.2 GB, so plan for a resumable transport; this path
   moves the file as raw bytes and has no base64 or whole-file upload API.

## Receiver steps

The receiver runs these on the Home that will keep the package, with the
Runtime's Kubo running. `DATA_DIR` is the Runtime data dir, by default
`~/.local/share/elastos` on Linux and `~/Library/Application Support/elastos`
on macOS.

1. Confirm the Runtime's Kubo is the pinned version with the canonical import
   profile, and install the platform llama.cpp engine if the Runtime does not
   already hold its verified receipt:

   ```bash
   python3 scripts/model-package-handoff.py verify-kubo --data-dir "$DATA_DIR"
   elastos setup --with llama-server
   ```

2. Check the CAR against its receipt before Kubo sees it. This recomputes the
   size and SHA-256 by streaming the file and refuses a truncated or altered
   CAR or a receipt for a different package:

   ```bash
   python3 scripts/model-package-handoff.py verify-car \
     --car qwen-package.car --receipt qwen-package.car.receipt.json \
     --cid bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi
   ```

3. Import and pin. The command repeats the CAR check, verifies Kubo, reads the
   Kubo repo path and refuses unless the free space left after the import stays
   at or above the floor you name. The floor is your budget and the command
   requires it. Kubo pins the root recursively during the import and the helper
   confirms the pin afterwards. Kubo answers only after the whole CAR has
   arrived and the root is pinned, so give the 6 GB import a timeout that
   covers that wait:

   ```bash
   python3 scripts/model-package-handoff.py import \
     --data-dir "$DATA_DIR" \
     --car qwen-package.car --receipt qwen-package.car.receipt.json \
     --cid bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi \
     --free-space-floor-bytes 21474836480 \
     --timeout-seconds 1800
   ```

   The receipt reports `"pinned": true` with the block count and block bytes
   Kubo stored. Running the same command again succeeds and leaves the pin in
   place.

4. Pin the catalog. First compare the handed-over `HEAD_CID` with the head of
   the bytes you received; `pin-catalog` refuses a mismatch as well:

   ```bash
   python3 scripts/model-package-handoff.py head-cid --catalog model-catalog.json
   ```

   The helper then checks the envelope, refuses a timed catalog, installs
   `model-catalog.json` into `DATA_DIR` at mode 0600 and replaces only the
   `model_catalog` key of `components.json`, leaving every other key as it
   was. The local Use budgets are yours to set and the Runtime rejects zero.
   Two floors apply to this package. The Qwen manifest declares
   `minimum_memory_mb` 8192, so `--max-model-memory-bytes` is at least
   8589934592. The Runtime charges the cache three times the payload plus
   8 MiB and 192 KiB of index allowance, which is 18516684377 bytes for the
   6169366387-byte payload, so `--max-cache-bytes` is at least that; the
   example uses 20 GiB:

   ```bash
   python3 scripts/model-package-handoff.py pin-catalog \
     --data-dir "$DATA_DIR" \
     --catalog model-catalog.json \
     --head-cid "$HEAD_CID" \
     --publisher-did "$PUBLISHER_DID" \
     --package-cid bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi \
     --max-cache-bytes 21474836480 \
     --max-model-memory-bytes 8589934592
   ```

   `DATA_DIR` has to be owned by you and closed to group and other writes,
   which is how the Runtime reads it as well.

5. Restart the Runtime with the platform restart helper from
   [HOME_LOCAL_SETUP.md](HOME_LOCAL_SETUP.md), then check readiness.
   `elastos content status --cid <package cid>` reports the package, and the
   System view in Home shows the model catalog as verified with the sender as
   publisher. Run local Use from Home as usual. That first successful Runtime
   read is the signature verification the `pin-catalog` receipt left pending.

If this Home already admitted the same package under a previous catalog head,
the admitted bytes and the original receipt stay in place, and readiness
follows the current head. Until you act, the Runtime reports the package as
admitted and not ready, and startup composition offers nothing for it. One
explicit same-CID Use under the new pin creates an alias record that reuses
the admitted bytes, so that Use completes without a second download and the
package becomes ready again.

## Failure handling

A truncated or corrupt CAR fails at `verify-car`, and `import` runs that same
check first, so Kubo receives no request for a bad file. If a transfer was
interrupted, copy the CAR again and rerun the same commands; every step
converges to the same end state, and a repeated `import` or `pin-catalog` is
safe.

If `import` refuses on the free-space floor, free space in the Kubo repo's
filesystem or choose a smaller floor and rerun. If Kubo reports an import error
mid-stream, the helper prints Kubo's message and the root stays unpinned.
Blocks that Kubo already wrote before the failure stay in its blockstore as
unpinned garbage until its next collection; a rerun with a complete CAR
deduplicates them, pins the root, and converges to the same end state. On the
synthetic closure a truncated import committed nothing; the helper does not
rely on that for larger packages.

The two pins come off independently. `kubo pin rm <package cid>` (with
`IPFS_PATH` set to the Runtime's Kubo repo) releases the retention pin and
leaves `components.json` alone. Removing the `model_catalog` key from
`components.json` and restarting the Runtime withdraws the catalog trust pin
and leaves the Kubo pin alone.

## Verification scope

`scripts/model-package-handoff-test.py` checks block-closure transfer, catalogue
trust and failure handling with isolated Kubo instances and test signing keys.
This operator utility does not replace installed Marketplace acquisition tests.
Current target verification and remaining work belong in [state.md](../state.md)
and [TASKS.md](../TASKS.md).

# Archive Manager Dependency And Release Policy

Date: 2026-06-06

This release enables Archive Manager only for archive families that already have
bounded provider-owned support in `object-provider`: `.zip`, `.tar`, `.tar.gz`,
and `.tgz`.

## Runtime Boundary

- Archive Manager is a viewer capsule. It does not parse archive bytes in the
  browser and does not call `/api/provider/object/*` directly.
- Runtime injects the signed principal and mediates viewer routes for archive
  stat, entry listing, safe entry preview, roots, and selected extraction.
- `object-provider` owns archive byte reads, path normalization, unsafe-entry
  rejection, selected writes, conflict policy, progress/cancel receipts, and
  WebSpace write-back policy.
- WebSpace archive reads use mounted `localhost://WebSpaces/...` handles. Any
  resolver-private `target_uri`, credentials, endpoint tokens, Kubo/IPFS handles,
  or host paths must be redacted from Archive Manager receipts.

## Enabled Families

- `.zip`: enabled for list, bounded safe entry preview, selected extract/import,
  folder download, and provider-owned ZIP creation.
- `.tar`: enabled for list, bounded safe entry preview, and selected
  extract/import.
- `.tar.gz` / `.tgz`: enabled for list, bounded safe entry preview, selected
  extract/import, and folder download.

All enabled families must use relative UTF-8 paths only. Absolute paths,
traversal, symlinks, devices, hardlinks, FIFOs, and other non-file entries are
blocked or rejected before write.

## Generic Family Review Gate

No generic non-tar/non-zip family is approved in this branch.

Before enabling a new family, the owner must record:

- Dependency license and redistribution posture.
- Maintenance status, upstream CVE posture, and release cadence.
- Memory and CPU bounds for listing, preview, and extraction.
- Streaming/listing support without unbounded in-memory expansion.
- Unsafe-entry handling equivalent to current ZIP/tar policy.
- Password/encrypted archive posture, including explicit fail-closed behavior.
- Runtime/build impact and platform support.
- Security owner approval.

## Current Decisions

- `.7z`: first candidate for future review, but not enabled in this branch.
- `.rar`: blocked until licensing, decompression safety, and redistribution are
  explicitly approved.
- `.tar.xz`, `.tar.bz2`, `.tar.zst`, `.xz`, `.bz2`, `.zst`, `.lz4`, and plain
  `.gz`: policy-gated until dependency and release review is complete.

Unsupported families remain visible as policy-gated archives in Library
Properties and Archive Manager. They can be inspected for object identity and
policy status, but entry browsing, preview, and extraction fail closed.

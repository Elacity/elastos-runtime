# Authentication Audit Chain

Runtime authentication state becomes audit-chain protected on its first
authority write. Audit events, chain links, retained anchors, chain state, and
the external activation checkpoint are signed by the Runtime Ed25519 identity.
Event and chain hashes remain SHA-256.

`audit-chain-required.json` uses
`elastos.audit.chain-activation/v2`. Its signed checkpoint records the current
chain head and retained-anchor boundary outside `auth-state.json`. A valid
signed auth state may advance that checkpoint after a crash between the two
atomic writes. It may not move the checkpoint backwards, replace a head at the
same sequence, remove a retained anchor, or substitute an anchor at the same
sequence.

Both files are written through private, unique temporary files, flushed before
rename, and followed by a parent-directory sync. Auth state, activation state,
and their lock files must be regular non-symlink files under regular
non-symlink auth directories.

## Compatibility Policy

There is no automatic legacy migration and no offline migration script in this
branch. An existing auth state that contains authority or audit history without
the signed chain state is preserved but rejected before expiry pruning. Back it
up and start with a fresh data root. Do not delete or rewrite the old state and
do not describe its pre-chain history as complete or verifiable.

A missing activation file is recoverable only from a fully verified signed
chain state, which closes the crash window between the auth-state and checkpoint
writes. A stale activation schema, invalid signature, retained-checkpoint
rollback, truncation, substitution, or unchained authority remains fail-closed.

This is a split-state local rollback witness, not trusted monotonic hardware or
an external transparency log. It detects an auth-state rollback while the
checkpoint is retained independently. A coordinated rollback or deletion of
the entire data root, or compromise of the Runtime signing key, requires an
operator-retained external backup or receipt to detect and is not claimed here.

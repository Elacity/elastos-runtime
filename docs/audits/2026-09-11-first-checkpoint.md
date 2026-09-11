# J1/C2 combined installed checkpoint

The combined automated checkpoint passes on Mac ARM64 and Linux x86_64.
AUTH-01 remains `Blocked prerequisite` because its required user-operated
passkey check is pending. Fresh isolated Homes are ready for that check. The
coordinator stops here; broader execution and recurring monitoring remain
paused. Mission R2, J1–J5, D1–D6 and the release gates retain their acceptance.
Jetson remains deferred.

## Candidate and review

The reviewed code candidate is `f21d285ec93e91b6a6b0ab0221e311668ecf77bf`, tree
`3a51a2562f011506678152d00453ccf420e2da41`. It is 51 commits ahead and zero behind
fetched `origin/upstream/0.7.1-dev` at `6c61c990`. Closeout documentation follows
the code candidate. Initial source was clean `bfe40ea0`, 49 ahead / zero behind.

One coordinator owned source edits, builds and tests on both targets. An
independent agent reviewed the relevant plan, consequential repair and evidence.
The older pinned assembly helper was reviewed and left unused. Each platform
built only its Runtime binary once for the revised candidate. Verified provider,
WASM and browser artifacts were reused.

## Acceptance and proof

| Criterion | Mac ARM64 installed Home | Linux x86_64 installed Home |
| --- | --- | --- |
| Recover opens kit selection immediately | Pass: rendered file chooser | Pass: rendered file chooser against Linux Runtime |
| Original Profile DID and exact name | Pass: same encrypted kit restores both; Home consumes the exact original name | Pass: same kit and checks |
| Wrong password and retry preserve existing data | Pass: Auth, Profile and key bytes preserved; retry completes | Pass: existing Auth, Profile, key and observed user-state bytes preserved; retry completes |
| Reload continuation | Pass: failed recovery → reload → sign-in → Home → System import; completed-kit retry after another sign-in | Pass: same continuation and completed-kit retry |
| Owned connections and processes close; lock and restart | Pass: gateway, local-control and managed-Runtime held connections close; all owned descendants exit, coordinates disappear and port releases; same Home restarts | Pass: equivalent target proof, including running `/proc` executable hash |
| Full Home setup and media reuse | Pass: two full Home setup runs skip media download and preserve tool bytes; separate corrupt-marker check rejects reuse | Pass: same full setup and integrity checks |
| Carrier lifecycle | Pass: installed transfer success/error, owned temporary socket cleanup on timeout, steady endpoint retained and fresh nonce delivered after peer resumes | Pass: equivalent installed checks |

Final shutdown after restart took **6.235 seconds on Mac** and **6.242 seconds
on Linux**. Initial integrated shutdown took 6.228 and 6.218 seconds. Each run
held three accepted HTTP requests, including the fully started subordinate
Runtime. Automated test gateways are stopped.

Browser tests used isolated Brave profiles with virtual CTAP2 authenticators
and actual installed APIs. Linux rendering was exercised from the Mac browser
through a private forward to the Linux Runtime. This proves the rendered Linux
service flow; a user-operated authenticator remains required by AUTH-01.
Its expected order was corrected before use and its pending verdict preserved.

The accepted recovery order remains kit → passkey → import. Reload uses the
existing sign-in/System continuation. The stable identity is the signed Profile
DID and name; a passkey controls access. Existing registered passkeys serve
sign-in, and recovery can establish a replacement. Validating the kit before
replacement enrollment remains a proposed UX improvement, pending a decision.

Exact internal Carrier borrowed-client and fetch-timeout cases retain the six
unchanged source lifecycle receipts. Installed checks add observable owned
connection cleanup, endpoint continuity and network reuse. They do not turn
those internal source cases into instrumented installed claims.

## Repairs and retained failures

`a45164cf` permits a fresh authenticated session for the same principal/root to
read its signed completed recovery receipt. Explicit terminal retry tokens keep
strict session/proof binding. Seven focused recovery tests pass, including
unrelated-principal and wrong-root rejection and single import/audit effects.

`f21d285e` binds subordinate process groups to their gateway generation and
awaits their cleanup before releasing ownership. Registration and shutdown
share a lock and atomic records; cancelled startup closes its child group.
Owner loss also covers Terminal-created subordinates. Focused tests cover held
HTTP requests, child/helper exit, cancellation, an exited group leader,
unrelated ownership and restart. Basic repository gates pass.

The earlier API-only shutdown passed, but the full Home journey exposed orphaned
managed Runtimes on both targets. Those failures are retained and superseded by
the integrated proof above. A broader gateway test selection also ran 749 cases:
746 passed; the child-reaping failure was repaired, while two unrelated custody
cases lacked their explicit provider fixture. The focused checkpoint gates pass;
this record makes no full-suite claim.

Fixture diagnostics are retained: missing archive/cache metadata, missing
capsule receipts on the media-only peer, a completed-receipt response assumption,
and Home window-state writes during a byte snapshot. Existing Profile/Auth/key
preservation is distinguished from ordinary asynchronous window persistence.
Full setup preserves the parsed manifest on repetition; serialized hashes can
change with map order. Initial setup also normalizes typed metadata. Current
receipts bind the resulting manifests rather than claiming byte stability.

## Artifact identities and closeout

| Target | Built and installed Runtime SHA-256 |
| --- | --- |
| Mac ARM64 | `318787304123e5371b340787ed3c56b6a1d66ca3bd54acc381edd43d973bb492` |
| Linux x86_64 | `6c3ef3ab7d404e223e8992bd69e0fd205cdbc9abc75aa767a7ffa80adb8fa261` |

Official source-home installation receipts were refreshed after setup and test
manifest changes. Evidence records source identity, built and installed paths
and hashes, component/capsule manifests, process identity and observed behavior.
Mac source cleanliness records documentation and Cargo lock ordering at the
installation time; Runtime source files match the frozen candidate. The lock
ordering change carries no dependency change. It is removed at closeout.

Resolve the private evidence root with
`git rev-parse --path-format=absolute --git-common-dir`. Its
`execution-pilot/checkpoint-bfe40ea0` directory contains reviewed plans, historical
failures, final per-target receipts and the resource ledger. The current
`development-loop-current.md` names the fresh human-test Homes and next action.
Compact receipts are retained; superseded automated fixture Homes are removed
under their recorded cleanup conditions. User Homes, keys, drafts and preview
artifacts remain outside those cleanup paths. The 70 branches and 12 worktrees
are preserved.

Only AUTH-01's real passkey action remains for this checkpoint. Existing preview
and public staging artifacts remain `d790a48e`; public live is unchanged. Public
deployment still needs exact-candidate review and Anders's approval. Final
installer stamping and signed packages remain after C5 source freeze. Duplicate
assistants, model loading and Browser loading remain the next priorities after
this bounded checkpoint, subject to a continuation request.

At the two 30-minute reports in the resumed run, account-wide weekly usage moved
54% → 56% → 57%. Credits were unchanged. These are shared account
measurements, not task-specific usage. New evidence advanced at both reports.

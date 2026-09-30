# Public preview deployment, 11 September 2026

The user-authorized preview is available at [ElastOS](https://elastos.elacitylabs.com/)
and [Home](https://elastos.elacitylabs.com/home/). The public Runtime restarted
at 17:42 UTC from `259666222f12b21283131cc7926ba7d0c1a52e99`, tree
`419bdba6a09ed37e8176ddfda9c22c1a270f6054`, published in
[draft PR64](https://github.com/Elacity/elastos-runtime/pull/64).

## Build and adoption

The first Linux build found an outdated standalone Object provider lockfile.
Its required packages already existed, with matching versions and checksums, in
the reviewed workspace lockfile. Commit `25966622` reconciles those inputs and
makes setup use `--locked`. All 25 required Linux manifests passed locked offline
metadata checks. The resumed build reused the compiled Runtime/providers and
completed source-home setup. Application source remains the reviewed `06bf4e0f` behavior.

The staged installation passed component integrity and Home/Services readiness.
Public adoption replaced the selected capsules, binaries, helpers, website and
manifests. Browser helper paths were bound to the stable public installation.
Verified media tools were reused through Runtime setup. Public Kubo, provider
settings, accounts, keys and user files stayed in place.

The initial check rejected an older group-writable identity directory. Its mode
was changed from `0775` to `0700`; Runtime created an empty owner-only identity
lock. Device keys and credentials retained their bytes. Before and after capsule
adoption, the migration guard reported `already_ready`, zero roots, zero objects
and an empty roots list. The restart migration receipt repeated that result.
Account/authentication records, user files and preserved configuration matched
the pre-start checks after startup.

## Verification

| Surface | SHA-256 |
| --- | --- |
| Built, installed and running Runtime | `b6634092055116c85556426f200c177fc6bf0ad2159af4025d48bb7bf3e89cff` |
| Installed components manifest | `7252c7363a264e16753907bcee98d36b96256865d3bd30b6cbd87db176606c8e` |
| Public website index | `d096d41c2e32beca1a9f2bb89ca7b69db343bd49739dbdcdbf61fc1c5af6c987` |
| Public Home index | `d2f5742661700f67814e6945cf4e6f6db4bed547cd949230be316f26dea3e34e` |

The installed integrity checker passed. All 573 selected artifact files matched
their staged inputs, including the intentional helper-path substitutions.
Five public HTTPS route checks passed: `/`, `/home/`, `/apps/services/` and
`/apps/assistant/` returned 200 with matching installed hashes; `/apps/home/`
redirected to `/home/` and returned the same bytes. A fresh
browser rendered the storefront and Home sign-in screen without console errors.
The target source was clean at verification. The private operator receipt holds
exact paths, commands, process identities and the full artifact inventory.

Recheck the public hashes with:

```bash
curl -fsSL https://elastos.elacitylabs.com/ | shasum -a 256
curl -fsSL https://elastos.elacitylabs.com/home/ | shasum -a 256
```

From the candidate checkout, the installed check uses `ELASTOS_DATA_DIR` set
to the verified installation directory:

```bash
python3 scripts/components-release-integrity-check.py \
  --manifest "$ELASTOS_DATA_DIR/components.json" --platform linux-amd64 \
  --profile source-home --source-root . \
  --source-home-data-dir "$ELASTOS_DATA_DIR"
```

## Remaining checks

Anders confirmed that existing-account sign-in and saved work look correct.
This closes the bounded deployment acceptance gate. The temporary stage and
146 MiB deployment rollback are removed. Earlier historical rollback sets retain
their separate reconciliation obligations.

Startup diagnostics report an unavailable configured Browser Engine and an
unconfigured inactive custody provider. J4/J5 acceptance remains open on the
required test targets. Full J1-J5 acceptance, Linux model qualification and
the signed three-platform installer release remain open. This deployment makes
the reviewed preview available for team testing; the download control correctly
states that the 0.7.1 installer is coming soon.

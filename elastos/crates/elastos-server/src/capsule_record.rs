//! Per-capsule signed records. A release lists one `elastos.capsule.record/v1`
//! envelope per published archive under its top-level `capsule_records`. Each
//! record is signed on its own and names only the archive's bytes and declared
//! capsule version, so it stays byte-identical while the capsule is unchanged.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::setup::PlatformInfo;

pub(crate) const DOMAIN: &str = "elastos.capsule.record.v1";
pub(crate) const SCHEMA: &str = "elastos.capsule.record/v1";
/// Releases before 0.8.0 list no records; this Runtime verifies records when a
/// release lists them. The next release sets this, and a release without
/// records is then refused.
pub(crate) const REQUIRED: bool = false;
/// A release is one 256 KiB chunk; each record takes about 600 bytes.
const MAX_RECORDS: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapsuleRecord {
    pub(crate) schema: String,
    pub(crate) name: String,
    pub(crate) platform: String,
    pub(crate) capsule_version: Option<String>,
    pub(crate) sha256: String,
    pub(crate) size: u64,
    pub(crate) cid: String,
    pub(crate) release_path: String,
    pub(crate) signer_did: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: serde_json::Value,
    signature: String,
    signer_did: String,
}

/// The verified records of one release, keyed by capsule name and release path.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CapsuleRecords(BTreeMap<(String, String), CapsuleRecord>);

impl CapsuleRecords {
    /// Verify every record a verified release payload lists, each against the
    /// accepted publisher DIDs. `None` when the release lists no records.
    pub(crate) fn from_release(
        release: &serde_json::Value,
        publisher_dids: &[String],
    ) -> Result<Option<Self>> {
        let Some(listed) = release.get("capsule_records") else {
            anyhow::ensure!(!REQUIRED, "The release lists no capsule records");
            return Ok(None);
        };
        let listed = listed
            .as_array()
            .context("Release capsule records must be a list")?;
        anyhow::ensure!(
            listed.len() <= MAX_RECORDS,
            "Release lists too many capsule records"
        );
        let mut records = BTreeMap::new();
        for envelope in listed {
            let record = verify(envelope, publisher_dids)?;
            let key = (record.name.clone(), record.release_path.clone());
            anyhow::ensure!(
                records.insert(key, record).is_none(),
                "Release lists a capsule record twice"
            );
        }
        Ok(Some(Self(records)))
    }

    fn get(&self, name: &str, release_path: Option<&str>) -> Option<&CapsuleRecord> {
        self.0.get(&(name.to_owned(), release_path?.to_owned()))
    }

    /// The signed record of this release row matches the row's checksum, size
    /// and CID. Bytes fetched for the row are then checked against the same values.
    pub(crate) fn admit(&self, name: &str, info: &PlatformInfo) -> Result<()> {
        let record = self
            .get(name, info.release_path.as_deref())
            .with_context(|| format!("The release has no signed capsule record for {name}"))?;
        let checksum = info
            .checksum
            .as_deref()
            .and_then(|value| value.strip_prefix("sha256:"));
        anyhow::ensure!(
            checksum == Some(record.sha256.as_str())
                && info.size == Some(record.size)
                && info.cid.as_deref() == Some(record.cid.as_str()),
            "The signed capsule record for {name} differs from the release components"
        );
        Ok(())
    }

    /// Every released row of a components manifest has exactly the record the
    /// release signer derives from it, and every record belongs to a row.
    pub(crate) fn admit_manifest(&self, manifest: &crate::setup::ComponentsManifest) -> Result<()> {
        // The signer lists records for `external` rows only and publishes an empty
        // capsule registry; a registry entry would be a fetchable row without one.
        anyhow::ensure!(
            manifest.capsules.is_empty(),
            "The release lists capsule records, so its capsule registry must be empty"
        );
        let mut used = std::collections::BTreeSet::new();
        for (name, component) in &manifest.external {
            let metadata = component
                .capsule_metadata
                .iter()
                .flat_map(|metadata| metadata.platforms.iter());
            for (platform, info) in component.platforms.iter().chain(metadata) {
                let Some(release_path) =
                    info.release_path.as_deref().filter(|path| !path.is_empty())
                else {
                    continue;
                };
                self.admit(name, info)?;
                let record = &self.0[&(name.clone(), release_path.to_owned())];
                anyhow::ensure!(
                    &record.platform == platform && record.capsule_version == component.version,
                    "The signed capsule record for {name} differs from the release components"
                );
                used.insert((name.clone(), release_path.to_owned()));
            }
        }
        anyhow::ensure!(
            used.len() == self.0.len(),
            "The release lists a capsule record no component row uses"
        );
        Ok(())
    }
}

fn verify(envelope: &serde_json::Value, publisher_dids: &[String]) -> Result<CapsuleRecord> {
    let envelope: Envelope =
        serde_json::from_value(envelope.clone()).context("Capsule record envelope refused")?;
    anyhow::ensure!(
        publisher_dids.iter().any(|did| did == &envelope.signer_did),
        "Capsule record is not signed by an accepted publisher"
    );
    // Release payloads are canonical: sorted keys, compact, as signed.
    let payload = serde_json::to_vec(&envelope.payload)?;
    crate::crypto::verify_domain_separated_signature(
        &envelope.signer_did,
        DOMAIN,
        &payload,
        &envelope.signature,
    )
    .context("Capsule record signature refused")?;
    let record: CapsuleRecord =
        serde_json::from_value(envelope.payload.clone()).context("Capsule record refused")?;
    // Every field is present, so the signed bytes and the record agree.
    anyhow::ensure!(
        serde_json::to_value(&record)? == envelope.payload,
        "Capsule record fields refused"
    );
    anyhow::ensure!(
        record.schema == SCHEMA && record.signer_did == envelope.signer_did,
        "Capsule record schema or signer refused"
    );
    anyhow::ensure!(
        !record.name.is_empty()
            && !record.platform.is_empty()
            && !record.release_path.is_empty()
            && record.size > 0
            && record.capsule_version.as_deref() != Some("")
            && record.sha256.len() == 64
            && record
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Capsule record fields refused"
    );
    cid::Cid::try_from(record.cid.as_str()).context("Capsule record CID refused")?;
    Ok(record)
}

/// The admission of one release's components: every row a Home may fetch from
/// it is checked here, and only rows of the admitted manifest are admitted.
/// Only this module constructs an `Admission`.
pub(crate) struct Admission {
    records: Option<CapsuleRecords>,
    signed: bool,
    /// The admitted manifest's fetchable rows; tests of planning may leave it unbound.
    rows: Option<ManifestRows>,
}

/// Every fetchable row of one manifest, as signed: `external` rows, their
/// capsule metadata rows, and capsule registry entries.
struct ManifestRows {
    external: BTreeMap<String, Vec<serde_json::Value>>,
    registry: BTreeMap<String, serde_json::Value>,
}

impl ManifestRows {
    fn of(manifest: &crate::setup::ComponentsManifest) -> Result<Self> {
        let mut external = BTreeMap::new();
        for (name, component) in &manifest.external {
            let metadata = component
                .capsule_metadata
                .iter()
                .flat_map(|metadata| metadata.platforms.values());
            let rows = component
                .platforms
                .values()
                .chain(metadata)
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?;
            external.insert(name.clone(), rows);
        }
        let registry = manifest
            .capsules
            .iter()
            .map(|(name, entry)| Ok((name.clone(), serde_json::to_value(entry)?)))
            .collect::<Result<_>>()?;
        Ok(Self { external, registry })
    }
}

/// One release row a caller may fetch and install. Only an `Admission`
/// constructs it, and every download and fetch of component bytes takes one,
/// so no acquisition path can skip admission.
#[derive(Clone)]
pub(crate) struct AdmittedRow {
    name: String,
    info: PlatformInfo,
}

impl AdmittedRow {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn info(&self) -> &PlatformInfo {
        &self.info
    }

    /// A row for transport tests that exercise the fetch path itself.
    #[cfg(test)]
    pub(crate) fn for_test(name: &str, info: &PlatformInfo) -> Self {
        Self {
            name: name.to_owned(),
            info: info.clone(),
        }
    }
}

impl Admission {
    /// A Home without an installed signed release (a source or CI Home): its
    /// local manifest carries no records to check. Refused on a Home that has
    /// a signed release, which must admit that release's records instead.
    pub(crate) fn unsigned_home(
        data_dir: &std::path::Path,
        manifest: &crate::setup::ComponentsManifest,
    ) -> Result<Self> {
        anyhow::ensure!(
            crate::setup::installed_release_source(data_dir)?.is_none(),
            "This Home has an installed signed release; its capsule records must be admitted"
        );
        Ok(Self {
            records: None,
            signed: false,
            rows: Some(ManifestRows::of(manifest)?),
        })
    }

    /// A signed release's admission with these records, for tests that
    /// exercise planning and staging.
    #[cfg(test)]
    pub(crate) fn listing(records: Option<CapsuleRecords>) -> Self {
        Self {
            records,
            signed: true,
            rows: None,
        }
    }

    pub(crate) fn records(&self) -> Option<&CapsuleRecords> {
        self.records.as_ref()
    }

    /// Admit one row of the admitted manifest before any of its bytes are
    /// fetched or written.
    pub(crate) fn row(&self, name: &str, info: &PlatformInfo) -> Result<AdmittedRow> {
        if let Some(rows) = &self.rows {
            let value = serde_json::to_value(info)?;
            anyhow::ensure!(
                rows.external
                    .get(name)
                    .is_some_and(|rows| rows.contains(&value)),
                "'{name}' is not a row of the admitted release"
            );
        }
        match &self.records {
            Some(records) => records.admit(name, info)?,
            None if REQUIRED && self.signed => {
                anyhow::bail!("The release lists no capsule records")
            }
            None => {}
        }
        Ok(AdmittedRow {
            name: name.to_owned(),
            info: info.clone(),
        })
    }

    /// Admit a capsule registry entry. Only a release without records (before
    /// 0.8.0) or a Home without a signed release can carry registry downloads.
    pub(crate) fn registry_entry(
        &self,
        name: &str,
        entry: &crate::setup::CapsuleEntry,
    ) -> Result<AdmittedRow> {
        anyhow::ensure!(
            self.records.is_none() && !(REQUIRED && self.signed),
            "The release lists capsule records, so its capsule registry must be empty"
        );
        if let Some(rows) = &self.rows {
            anyhow::ensure!(
                rows.registry.get(name) == Some(&serde_json::to_value(entry)?),
                "'{name}' is not a registry entry of the admitted release"
            );
        }
        anyhow::ensure!(
            entry.sha256.len() == 64
                && entry
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "No sha256 checksum for capsule '{name}'. Integrity verification is mandatory."
        );
        let info = serde_json::from_value(serde_json::json!({
            "cid": entry.cid, "checksum": format!("sha256:{}", entry.sha256),
            // Older registries leave size unset (0); the checksum still binds the bytes.
            "size": (entry.size > 0).then_some(entry.size),
        }))?;
        Ok(AdmittedRow {
            name: name.to_owned(),
            info,
        })
    }
}

/// Admit a signed release's components for acquisition. The release envelope
/// must verify against an accepted publisher DID, `components` must be the exact
/// bytes its `release_platform` descriptor binds, and every row the release can
/// serve must have exactly its signed record (optional before records are
/// required). This is the only way to admit rows of a signed release.
pub(crate) fn admit_release(
    release: &[u8],
    publisher_dids: &[String],
    components: &[u8],
    release_platform: &str,
) -> Result<(crate::setup::ComponentsManifest, Admission)> {
    use sha2::Digest as _;
    let (envelope, _) = crate::crypto::verify_release_envelope_against_dids(
        release,
        "elastos.release.v1",
        publisher_dids,
    )?;
    let payload = &envelope["payload"];
    crate::installed_release::admit_descriptor(
        &payload["platforms"][release_platform]["components"],
        &hex::encode(sha2::Sha256::digest(components)),
        components.len() as u64,
    )?;
    let manifest: crate::setup::ComponentsManifest = serde_json::from_slice(components)?;
    let admission = admit_components(
        CapsuleRecords::from_release(payload, publisher_dids)?,
        &manifest,
    )?;
    Ok((manifest, admission))
}

/// Admit a release's whole components manifest against its records before any
/// capsule of it is written: every row the release can serve, replaced or kept,
/// has exactly its signed record. A release without records is admitted only
/// while records are optional.
fn admit_components(
    records: Option<CapsuleRecords>,
    manifest: &crate::setup::ComponentsManifest,
) -> Result<Admission> {
    match &records {
        Some(records) => records.admit_manifest(manifest)?,
        None if REQUIRED => anyhow::bail!("The release lists no capsule records"),
        None => {}
    }
    Ok(Admission {
        records,
        signed: true,
        rows: Some(ManifestRows::of(manifest)?),
    })
}

/// Records of the installed release and the admission of the release an
/// update installs.
pub(crate) struct UpdateRecords {
    pub(crate) installed: Option<CapsuleRecords>,
    pub(crate) next: Admission,
}

#[cfg(test)]
impl Default for UpdateRecords {
    /// A signed release without records on both sides (before 0.8.0).
    fn default() -> Self {
        Self {
            installed: None,
            next: Admission::listing(None),
        }
    }
}

impl UpdateRecords {
    /// `next` is the admission of the release being installed. The installed
    /// release's records are read only to compare, so an unreadable installed
    /// list makes every row count as changed by its description alone.
    pub(crate) fn new(
        installed_release: &[u8],
        next: Admission,
        publisher_dids: &[String],
    ) -> Self {
        let installed = serde_json::from_slice::<serde_json::Value>(installed_release)
            .ok()
            .and_then(|release| {
                CapsuleRecords::from_release(&release["payload"], publisher_dids)
                    .ok()
                    .flatten()
            });
        Self { installed, next }
    }

    /// When both releases list records, a row whose record differs is replaced
    /// even if its components description did not change.
    pub(crate) fn changed(
        &self,
        name: &str,
        installed: Option<&PlatformInfo>,
        next: &PlatformInfo,
    ) -> bool {
        let (Some(old), Some(new)) = (&self.installed, self.next.records()) else {
            return false;
        };
        old.get(
            name,
            installed.and_then(|info| info.release_path.as_deref()),
        ) != new.get(name, next.release_path.as_deref())
    }
}

/// A record envelope signed by `key`, as the release signer writes it.
#[cfg(test)]
pub(crate) fn signed_record(
    key: &elastos_runtime::signature::SigningKey,
    payload: serde_json::Value,
) -> serde_json::Value {
    let (signature, signer_did) =
        crate::crypto::domain_separated_sign(key, DOMAIN, &serde_json::to_vec(&payload).unwrap());
    serde_json::json!({"payload": payload, "signature": signature, "signer_did": signer_did})
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/capsule-record.json");

    fn fixture() -> (serde_json::Value, String) {
        let envelope: serde_json::Value = serde_json::from_slice(FIXTURE).unwrap();
        let did = envelope["signer_did"].as_str().unwrap().to_owned();
        (envelope, did)
    }

    fn row(record: &serde_json::Value) -> PlatformInfo {
        serde_json::from_value(serde_json::json!({
            "checksum": format!("sha256:{}", record["sha256"].as_str().unwrap()),
            "size": record["size"], "cid": record["cid"], "release_path": record["release_path"],
            "install_path": "capsules/system", "extract_path": "system"
        }))
        .unwrap()
    }

    fn listed(records: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({"schema": "elastos.release/v1", "capsule_records": records})
    }

    #[test]
    fn release_signer_fixture_verifies_and_admits_its_row() {
        let (envelope, did) = fixture();
        let records = CapsuleRecords::from_release(&listed(vec![envelope.clone()]), &[did])
            .unwrap()
            .unwrap();
        records.admit("system", &row(&envelope["payload"])).unwrap();
    }

    #[test]
    fn records_need_their_own_publisher_signature() {
        let (envelope, did) = fixture();
        let mut forged = envelope.clone();
        let signature = forged["signature"].as_str().unwrap();
        let flipped = if signature.starts_with('0') { "1" } else { "0" };
        forged["signature"] = serde_json::json!(format!("{flipped}{}", &signature[1..]));
        let error = CapsuleRecords::from_release(&listed(vec![forged]), std::slice::from_ref(&did))
            .unwrap_err();
        assert!(format!("{error:#}").contains("Capsule record signature refused"));

        // The signature covers the payload: a changed size is refused.
        let mut tampered = envelope.clone();
        tampered["payload"]["size"] = serde_json::json!(38);
        let error = CapsuleRecords::from_release(&listed(vec![tampered]), &[did]).unwrap_err();
        assert!(format!("{error:#}").contains("Capsule record signature refused"));

        // A record signed by a key the Home does not accept is refused, even when it verifies.
        let (other, _) = elastos_runtime::signature::generate_keypair();
        let other_did = crate::crypto::encode_signing_key_did(&other);
        let error = CapsuleRecords::from_release(
            &listed(vec![envelope.clone()]),
            std::slice::from_ref(&other_did),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("not signed by an accepted publisher"));
        let mut payload = envelope["payload"].clone();
        payload["signer_did"] = serde_json::json!(other_did);
        let error = CapsuleRecords::from_release(
            &listed(vec![signed_record(&other, payload)]),
            &[fixture().1],
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("not signed by an accepted publisher"));
    }

    #[test]
    fn signed_records_must_match_the_row_they_admit() {
        let (envelope, did) = fixture();
        let records = CapsuleRecords::from_release(&listed(vec![envelope.clone()]), &[did])
            .unwrap()
            .unwrap();
        let payload = &envelope["payload"];
        let other = format!("sha256:{}", "0".repeat(64));
        for (field, change) in [
            ("size", serde_json::json!(36)),
            ("checksum", serde_json::json!(other)),
            (
                "cid",
                serde_json::json!("QmQE64AZECkdQfesc8DfxbrC2aHif5ftTqwdXc3EqqPUmj"),
            ),
        ] {
            let mut info = serde_json::to_value(row(payload)).unwrap();
            info[field] = change;
            let error = records
                .admit("system", &serde_json::from_value(info).unwrap())
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("differs from the release components"),
                "{field}"
            );
        }
        // A row the release has no record for is refused, by name and by path.
        assert!(records.admit("services", &row(payload)).is_err());
        let mut info = serde_json::to_value(row(payload)).unwrap();
        info["release_path"] = serde_json::json!("system-v2.tar.gz");
        let error = records
            .admit("system", &serde_json::from_value(info).unwrap())
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("no signed capsule record for system"));
    }

    #[test]
    fn record_shape_is_exact_and_listed_once() {
        let (key, _) = elastos_runtime::signature::generate_keypair();
        let did = crate::crypto::encode_signing_key_did(&key);
        let payload = |extra: serde_json::Value| {
            let mut payload = serde_json::json!({
                "schema": SCHEMA, "name": "system", "platform": "*", "capsule_version": null,
                "sha256": "7cf3f4bff5fcfb9e2586e73f7d5ac35bef446db8e7fc6987b7064d31a7f7480d",
                "size": 37, "cid": "bafkreid46p2l75p47opclbxhh56vvq2355cg3ohh7ruypnygjuy2p52ibu",
                "release_path": "system.tar.gz", "signer_did": did
            });
            payload
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            payload
        };
        let accepted = signed_record(&key, payload(serde_json::json!({})));
        assert!(CapsuleRecords::from_release(
            &listed(vec![accepted.clone()]),
            std::slice::from_ref(&did)
        )
        .is_ok());
        for (case, extra) in [
            (
                "unknown field",
                serde_json::json!({"release_version": "0.8.0"}),
            ),
            (
                "schema",
                serde_json::json!({"schema": "elastos.capsule.record/v2"}),
            ),
            ("hash", serde_json::json!({"sha256": "ABC"})),
            ("size", serde_json::json!({"size": 0})),
            ("cid", serde_json::json!({"cid": "not-a-cid"})),
        ] {
            let record = signed_record(&key, payload(extra));
            assert!(
                CapsuleRecords::from_release(&listed(vec![record]), std::slice::from_ref(&did))
                    .is_err(),
                "{case}"
            );
        }
        let mut missing = payload(serde_json::json!({}));
        missing.as_object_mut().unwrap().remove("capsule_version");
        assert!(CapsuleRecords::from_release(
            &listed(vec![signed_record(&key, missing)]),
            std::slice::from_ref(&did)
        )
        .is_err());
        let error = CapsuleRecords::from_release(&listed(vec![accepted.clone(), accepted]), &[did])
            .unwrap_err();
        assert!(error.to_string().contains("twice"));
    }

    #[test]
    fn every_released_row_needs_exactly_its_record_even_when_unchanged() {
        let (envelope, did) = fixture();
        let records = CapsuleRecords::from_release(&listed(vec![envelope.clone()]), &[did])
            .unwrap()
            .unwrap();
        let row = serde_json::to_value(row(&envelope["payload"])).unwrap();
        let manifest = |external: serde_json::Value| -> crate::setup::ComponentsManifest {
            serde_json::from_value(serde_json::json!({"profiles": {}, "external": external}))
                .unwrap()
        };
        records
            .admit_manifest(&manifest(
                serde_json::json!({"system": {"platforms": {"*": row}}}),
            ))
            .unwrap();
        let mut sized = row.clone();
        sized["size"] = serde_json::json!(38);
        let other = serde_json::json!({"platforms": {"*": {
            "release_path": "services.tar.gz", "checksum": format!("sha256:{}", "1".repeat(64)),
            "size": 1, "cid": "bafkreid46p2l75p47opclbxhh56vvq2355cg3ohh7ruypnygjuy2p52ibu"}}});
        for (case, external, refusal) in [
            (
                "size differs from an unchanged record",
                serde_json::json!({"system": {"platforms": {"*": sized}}}),
                "differs from the release components",
            ),
            (
                "a row without a record",
                serde_json::json!({"system": {"platforms": {"*": row}}, "services": other}),
                "no signed capsule record for services",
            ),
            (
                "another platform key",
                serde_json::json!({"system": {"platforms": {"darwin-arm64": row}}}),
                "differs from the release components",
            ),
            (
                "another declared version",
                serde_json::json!({"system": {"version": "2", "platforms": {"*": row}}}),
                "differs from the release components",
            ),
            (
                "a record without a row",
                serde_json::json!({}),
                "no component row uses",
            ),
        ] {
            let error = records.admit_manifest(&manifest(external)).unwrap_err();
            assert!(error.to_string().contains(refusal), "{case}: {error:#}");
        }
    }

    #[test]
    fn a_release_listing_records_admits_no_capsule_registry_download() {
        let (envelope, did) = fixture();
        let entry: crate::setup::CapsuleEntry = serde_json::from_value(serde_json::json!({
            "cid": "bafkreid46p2l75p47opclbxhh56vvq2355cg3ohh7ruypnygjuy2p52ibu",
            "sha256": "1".repeat(64), "size": 1
        }))
        .unwrap();
        let registry: crate::setup::ComponentsManifest =
            serde_json::from_value(serde_json::json!({
                "profiles": {}, "external": {}, "capsules": {"fixture-app": entry}
            }))
            .unwrap();
        // An empty record list with a registry entry and no external rows is refused.
        let empty =
            CapsuleRecords::from_release(&listed(vec![]), std::slice::from_ref(&did)).unwrap();
        let error = admit_components(empty, &registry).err().unwrap();
        assert!(error.to_string().contains("capsule registry must be empty"));
        let listing = CapsuleRecords::from_release(&listed(vec![envelope]), &[did]).unwrap();
        let error = Admission::listing(listing)
            .registry_entry("fixture-app", &entry)
            .err()
            .unwrap();
        assert!(error.to_string().contains("capsule registry must be empty"));
        // A release before records, or a Home without a signed release, keeps
        // registry downloads, each with its signed checksum.
        assert!(admit_components(None, &registry)
            .unwrap()
            .registry_entry("fixture-app", &entry)
            .is_ok());
        let mut unsigned = entry.clone();
        unsigned.sha256.clear();
        let local: crate::setup::ComponentsManifest = serde_json::from_value(serde_json::json!({
            "profiles": {}, "external": {}, "capsules": {"fixture-app": unsigned}
        }))
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let admission = Admission::unsigned_home(home.path(), &local).unwrap();
        assert!(admission.registry_entry("fixture-app", &unsigned).is_err());
        // Only rows of the admitted manifest are admitted.
        let error = admit_components(None, &registry)
            .unwrap()
            .registry_entry("fixture-app", &unsigned)
            .err()
            .unwrap();
        assert!(error
            .to_string()
            .contains("not a registry entry of the admitted release"));
        let error = admission
            .row(
                "fixture-app",
                &serde_json::from_value(serde_json::json!({})).unwrap(),
            )
            .err()
            .unwrap();
        assert!(error
            .to_string()
            .contains("not a row of the admitted release"));
    }

    #[test]
    fn releases_without_records_stay_admitted_until_records_are_required() {
        let (envelope, did) = fixture();
        let release = serde_json::json!({"schema": "elastos.release/v1"});
        assert_eq!(
            CapsuleRecords::from_release(&release, &[did]).unwrap(),
            None
        );
        let manifest: crate::setup::ComponentsManifest = serde_json::from_value(serde_json::json!({
            "profiles": {}, "external": {"system": {"platforms": {"*": row(&envelope["payload"])}}}
        }))
        .unwrap();
        assert!(admit_components(None, &manifest)
            .unwrap()
            .row("system", &row(&envelope["payload"]))
            .is_ok());
    }

    #[test]
    fn rows_are_admitted_only_from_a_verified_release_and_the_components_it_binds() {
        use sha2::Digest as _;
        let (key, _) = elastos_runtime::signature::generate_keypair();
        let did = crate::crypto::encode_signing_key_did(&key);
        let components = br#"{"profiles":{},"external":{}}"#.to_vec();
        let multihash =
            cid::multihash::Multihash::<64>::wrap(0x12, &sha2::Sha256::digest(&components))
                .unwrap();
        let payload = serde_json::json!({"schema": "elastos.release/v1", "capsule_records": [],
            "platforms": {"aarch64-darwin": {"components": {
                "cid": cid::Cid::new_v1(0x55, multihash).to_string(),
                "sha256": hex::encode(sha2::Sha256::digest(&components)),
                "size": components.len()}}}});
        let sign = |key: &elastos_runtime::signature::SigningKey, payload: &serde_json::Value| {
            let (signature, signer_did) = crate::crypto::domain_separated_sign(
                key,
                "elastos.release.v1",
                &serde_json::to_vec(payload).unwrap(),
            );
            serde_json::to_vec(
                &serde_json::json!({"payload": payload, "signature": signature, "signer_did": signer_did}),
            )
            .unwrap()
        };
        let release = sign(&key, &payload);
        let dids = std::slice::from_ref(&did);
        assert!(admit_release(&release, dids, &components, "aarch64-darwin").is_ok());
        // Another publisher's release, other components bytes, or another
        // platform's descriptor admit nothing.
        let (other, _) = elastos_runtime::signature::generate_keypair();
        assert!(
            admit_release(&sign(&other, &payload), dids, &components, "aarch64-darwin").is_err()
        );
        let mut altered = components.clone();
        altered.push(b'\n');
        assert!(admit_release(&release, dids, &altered, "aarch64-darwin").is_err());
        assert!(admit_release(&release, dids, &components, "x86_64-linux").is_err());
    }
}

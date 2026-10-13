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

/// Records of the installed release and of the release an update installs.
#[derive(Default)]
pub(crate) struct UpdateRecords {
    pub(crate) installed: Option<CapsuleRecords>,
    pub(crate) next: Option<CapsuleRecords>,
}

impl UpdateRecords {
    /// `next` holds the verified records of the release being installed. The
    /// installed release's records are read only to compare, so an unreadable
    /// installed list makes every row count as changed by its description alone.
    pub(crate) fn new(
        installed_release: &[u8],
        next: Option<CapsuleRecords>,
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
        let (Some(old), Some(new)) = (&self.installed, &self.next) else {
            return false;
        };
        old.get(
            name,
            installed.and_then(|info| info.release_path.as_deref()),
        ) != new.get(name, next.release_path.as_deref())
    }

    /// Admit a row before any of its bytes are fetched or written.
    pub(crate) fn admit(&self, name: &str, info: &PlatformInfo) -> Result<()> {
        admit(self.next.as_ref(), name, info)
    }
}

/// Admit a release row against the release's records; a release without
/// records is admitted only while records are optional.
pub(crate) fn admit(
    records: Option<&CapsuleRecords>,
    name: &str,
    info: &PlatformInfo,
) -> Result<()> {
    match records {
        Some(records) => records.admit(name, info),
        None if REQUIRED => anyhow::bail!("The release lists no capsule records"),
        None => Ok(()),
    }
}

/// Admit a release's whole components manifest against its records before any
/// capsule of it is written: every released row has exactly its signed record,
/// including rows an update leaves in place.
pub(crate) fn admit_components(
    records: Option<&CapsuleRecords>,
    manifest: &crate::setup::ComponentsManifest,
) -> Result<()> {
    match records {
        Some(records) => records.admit_manifest(manifest),
        None if REQUIRED => anyhow::bail!("The release lists no capsule records"),
        None => Ok(()),
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
        admit_components(
            Some(&records),
            &manifest(serde_json::json!({"system": {"platforms": {"*": row}}})),
        )
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
            let error = admit_components(Some(&records), &manifest(external)).unwrap_err();
            assert!(error.to_string().contains(refusal), "{case}: {error:#}");
        }
    }

    #[test]
    fn releases_without_records_stay_admitted_until_records_are_required() {
        let (envelope, did) = fixture();
        let release = serde_json::json!({"schema": "elastos.release/v1"});
        assert_eq!(
            CapsuleRecords::from_release(&release, &[did]).unwrap(),
            None
        );
        assert!(admit(None, "system", &row(&envelope["payload"])).is_ok());
    }
}

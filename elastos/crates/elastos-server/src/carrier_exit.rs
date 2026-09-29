//! Runtime-owned admission for a peer's use of a locally issued Browser Exit grant.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde_json::Value;

pub(crate) const EXIT_GRANT_TTL_SECS: u64 = 60 * 60;
// A page uses parallel website and browser-service connections. Keep admission
// bounded per approved person and across the host, rather than limiting a page
// to the two streams used by the original transport fixture.
pub(crate) const EXIT_GRANT_MAX_STREAMS: usize = 64;
const EXIT_HOST_MAX_STREAMS: usize = 256;
const SERVICE_MESSAGE_DOMAIN: &str = "elastos.services.access-message.v1";
const MAX_SERVICE_MESSAGE_BYTES: usize = 16 * 1024;

pub(crate) fn sign_service_message(data_dir: &std::path::Path, payload: &mut Value) -> Result<()> {
    let (key, did) =
        crate::collaboration_profile_authority::load_existing_device_signing_key(data_dir)?
            .context("Services requires the existing Runtime signing identity")?;
    let object = payload
        .as_object_mut()
        .context("Services message must be an object")?;
    object.remove("signature");
    object.insert("signer_did".into(), Value::String(did));
    let bytes = serde_json::to_vec(payload)?;
    anyhow::ensure!(
        bytes.len() <= MAX_SERVICE_MESSAGE_BYTES,
        "Services message is too large"
    );
    let (signature, _) = crate::crypto::domain_separated_sign(&key, SERVICE_MESSAGE_DOMAIN, &bytes);
    payload["signature"] = Value::String(signature);
    Ok(())
}

pub(crate) fn verify_service_message(message: &Value, peer_field: &str) -> Result<Value> {
    let content = message["content"]
        .as_str()
        .context("Services message content required")?;
    anyhow::ensure!(
        content.len() <= MAX_SERVICE_MESSAGE_BYTES,
        "Services message is too large"
    );
    let mut payload: Value = serde_json::from_str(content)?;
    let signature = payload
        .as_object_mut()
        .context("Services message must be an object")?
        .remove("signature")
        .and_then(|value| value.as_str().map(str::to_string))
        .context("Services message signature required")?;
    let did = payload["signer_did"]
        .as_str()
        .context("Services signer required")?;
    let endpoint = super::did_to_public_key(did)
        .context("Services signer is invalid")?
        .to_string();
    anyhow::ensure!(
        payload[peer_field].as_str() == Some(&endpoint)
            && message["sender_id"].as_str() == Some(&endpoint),
        "Services message sender does not match signer"
    );
    crate::crypto::verify_domain_separated_signature(
        did,
        SERVICE_MESSAGE_DOMAIN,
        &serde_json::to_vec(&payload)?,
        &signature,
    )?;
    payload["signature"] = Value::String(signature);
    Ok(payload)
}

#[derive(Clone)]
pub(super) struct BrowserExitAuthority {
    pub data_dir: std::path::PathBuf,
    pub network: crate::collaboration_network::VerifiedCollaborationNetworkProfile,
    pub source: iroh::PublicKey,
    pub request: Value,
}

impl BrowserExitAuthority {
    pub async fn read(&self) -> Result<BrowserExitGrant> {
        let authority = self.clone();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio::task::spawn_blocking(move || {
                crate::api::gateway::authorize_home_service_exit(
                    &authority.data_dir,
                    &authority.network,
                    &authority.source,
                    &authority.request,
                    crate::auth::now_ts(),
                )
            }),
        )
        .await
        .context("Exit authority check deadline")??
    }

    pub async fn until_revoked(&self, original: &BrowserExitGrant) -> Result<()> {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            anyhow::ensure!(self.read().await? == *original, "Exit grant changed");
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrowserExitGrant {
    pub provider_principal_id: String,
    pub requester_principal_id: String,
    pub requester_endpoint: iroh::PublicKey,
    pub grant_id: String,
    pub revision: u64,
    pub expires_at: u64,
    pub max_active_streams: usize,
    pub max_active_streams_per_principal: usize,
}

impl BrowserExitGrant {
    pub fn validate(&self, source: &iroh::PublicKey, request: &Value, now: u64) -> Result<()> {
        anyhow::ensure!(
            self.max_active_streams > 0
                && self.max_active_streams <= EXIT_GRANT_MAX_STREAMS
                && self.max_active_streams_per_principal > 0
                && self.max_active_streams_per_principal <= self.max_active_streams,
            "Browser Exit stream limits are invalid"
        );
        anyhow::ensure!(
            self.requester_endpoint == *source
                && request["principal_id"].as_str() == Some(&self.requester_principal_id)
                && request["grant_id"].as_str() == Some(&self.grant_id),
            "Browser Exit grant does not authorize this requester"
        );
        anyhow::ensure!(
            self.revision > 0
                && self.revision <= now.saturating_add(30)
                && self.expires_at > now
                && self.expires_at > self.revision
                && self.expires_at - self.revision <= EXIT_GRANT_TTL_SECS,
            "Browser Exit grant is expired or invalid"
        );
        let target = request["target"]
            .as_str()
            .context("Browser Exit target required")?;
        anyhow::ensure!(target.len() <= 2048, "Browser Exit target is too long");
        let url = url::Url::parse(target)?;
        anyhow::ensure!(
            matches!(url.scheme(), "tcp" | "tls")
                && matches!(url.port(), Some(80 | 443))
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && matches!(url.path(), "" | "/")
                && url.query().is_none()
                && url.fragment().is_none(),
            "Browser Exit target is outside the issued grant"
        );
        let stream = request["stream_id"]
            .as_str()
            .context("Browser Exit stream id required")?;
        anyhow::ensure!(
            !stream.is_empty()
                && stream.len() <= 256
                && stream
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_')),
            "Browser Exit stream id is invalid"
        );
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(crate) struct BrowserExitReservations(Arc<Mutex<BTreeMap<(String, String), BrowserExitGrant>>>);

pub(crate) struct BrowserExitReservation {
    state: BrowserExitReservations,
    key: (String, String),
}

impl BrowserExitReservations {
    pub fn reserve(
        &self,
        grant: &BrowserExitGrant,
        stream_id: &str,
    ) -> Result<BrowserExitReservation> {
        let mut active = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Browser Exit accounting unavailable"))?;
        let key = (grant.grant_id.clone(), stream_id.to_string());
        anyhow::ensure!(
            !active.contains_key(&key),
            "Browser Exit stream is already active"
        );
        anyhow::ensure!(
            active.len() < EXIT_HOST_MAX_STREAMS
                && active
                    .values()
                    .filter(|value| value.grant_id == grant.grant_id)
                    .count()
                    < grant.max_active_streams
                && active
                    .values()
                    .filter(|value| value.requester_endpoint == grant.requester_endpoint
                        && value.requester_principal_id == grant.requester_principal_id)
                    .count()
                    < grant.max_active_streams_per_principal,
            "Browser Exit stream quota is exhausted"
        );
        active.insert(key.clone(), grant.clone());
        Ok(BrowserExitReservation {
            state: self.clone(),
            key,
        })
    }
}

impl Drop for BrowserExitReservation {
    fn drop(&mut self) {
        if let Ok(mut active) = self.state.0.lock() {
            active.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (BrowserExitGrant, Value) {
        let endpoint = iroh::SecretKey::from_bytes(&[41; 32]).public();
        (
            BrowserExitGrant {
                provider_principal_id: "person:provider".into(),
                requester_principal_id: "person:requester".into(),
                requester_endpoint: endpoint,
                grant_id: "issued-grant".into(),
                revision: 100,
                expires_at: 200,
                max_active_streams: 4,
                max_active_streams_per_principal: 2,
            },
            json!({"grant_id":"issued-grant", "principal_id":"person:requester",
            "target":"tls://example.com:443", "stream_id":"stream:1"}),
        )
    }

    #[test]
    fn authenticated_endpoint_grant_principal_lifetime_and_target_are_required() {
        let (grant, request) = fixture();
        grant
            .validate(&grant.requester_endpoint, &request, 150)
            .unwrap();
        assert!(grant
            .validate(
                &iroh::SecretKey::from_bytes(&[42; 32]).public(),
                &request,
                150
            )
            .is_err());
        for (field, value) in [
            ("grant_id", "other"),
            ("principal_id", "person:forged"),
            ("target", "tcp://example.com:22"),
            ("target", "tls://user@example.com:443"),
            ("stream_id", "bad/stream"),
        ] {
            let mut forged = request.clone();
            forged[field] = json!(value);
            assert!(grant
                .validate(&grant.requester_endpoint, &forged, 150)
                .is_err());
        }
        assert!(grant
            .validate(&grant.requester_endpoint, &request, 200)
            .is_err());
        let mut unbounded = grant.clone();
        unbounded.expires_at = 100 + EXIT_GRANT_TTL_SECS + 1;
        assert!(unbounded
            .validate(&grant.requester_endpoint, &request, 150)
            .is_err());
    }

    #[test]
    fn reservations_bound_concurrent_opens_and_release_on_cancel() {
        let (grant, _) = fixture();
        let state = BrowserExitReservations::default();
        let first = state.reserve(&grant, "stream:1").unwrap();
        assert!(state.reserve(&grant, "stream:1").is_err());
        let second = state.reserve(&grant, "stream:2").unwrap();
        assert!(state.reserve(&grant, "stream:3").is_err());
        drop(first);
        let third = state.reserve(&grant, "stream:3").unwrap();
        drop((second, third));
        assert!(state.0.lock().unwrap().is_empty());
    }

    #[test]
    fn normal_page_parallel_streams_remain_bounded_and_release_capacity() {
        let (mut grant, _) = fixture();
        grant.max_active_streams = EXIT_GRANT_MAX_STREAMS;
        grant.max_active_streams_per_principal = EXIT_GRANT_MAX_STREAMS;
        let state = BrowserExitReservations::default();
        let mut streams = (0..EXIT_GRANT_MAX_STREAMS)
            .map(|n| state.reserve(&grant, &format!("stream:{n}")).unwrap())
            .collect::<Vec<_>>();
        assert!(state.reserve(&grant, "stream:overflow").is_err());
        let mut another_grant = grant.clone();
        another_grant.grant_id = "another-grant".into();
        assert!(state.reserve(&another_grant, "stream:other-grant").is_err());
        streams.pop();
        let replacement = state.reserve(&grant, "stream:replacement").unwrap();
        drop((streams, replacement));
        assert!(state.0.lock().unwrap().is_empty());
        let mut all = Vec::new();
        for person in 0..4 {
            grant.requester_principal_id = format!("person:{person}");
            grant.grant_id = format!("grant:{person}");
            for n in 0..EXIT_GRANT_MAX_STREAMS {
                all.push(state.reserve(&grant, &format!("stream:{n}")).unwrap());
            }
        }
        grant.requester_principal_id = "person:overflow".into();
        grant.grant_id = "grant:overflow".into();
        assert!(state.reserve(&grant, "stream:overflow").is_err());
        drop(all);
        assert!(state.reserve(&grant, "stream:after-close").is_ok());
        assert!(state.0.lock().unwrap().is_empty());
    }

    #[test]
    fn service_envelopes_reject_forgery_relay_substitution_and_payload_changes() {
        let root = tempfile::tempdir().unwrap();
        let (_, did) = elastos_identity::load_or_create_did(root.path()).unwrap();
        let peer = crate::carrier::did_to_public_key(&did).unwrap().to_string();
        let mut payload = json!({"provider_peer_id":peer, "decision":"approved", "created_at":100});
        sign_service_message(root.path(), &mut payload).unwrap();
        let mut message = json!({"sender_id":peer, "content":payload.to_string()});
        assert_eq!(
            verify_service_message(&message, "provider_peer_id").unwrap(),
            payload
        );
        message["sender_id"] = json!("other");
        assert!(verify_service_message(&message, "provider_peer_id").is_err());
        message["sender_id"] = json!(peer);
        payload["decision"] = json!("denied");
        message["content"] = json!(payload.to_string());
        assert!(verify_service_message(&message, "provider_peer_id").is_err());
        payload.as_object_mut().unwrap().remove("signature");
        message["content"] = json!(payload.to_string());
        assert!(verify_service_message(&message, "provider_peer_id").is_err());
    }
}

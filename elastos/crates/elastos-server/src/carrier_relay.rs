//! The one Carrier relay.
//!
//! Every Home accepts exactly one relay: its trusted publisher's origin, the
//! `PUBLISHER_GATEWAY` already pinned in the signed release and stamped into
//! `install.sh`. A Home stays connected to it, dials a peer by node ID alone
//! (the relay is attached here, not by callers) and still prefers a direct
//! path. Any other relay is refused. The relay sees only end-to-end
//! encrypted QUIC between keys it cannot impersonate.
//!
//! The seed's public role also runs the relay server, behind its TLS proxy.

use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use futures_lite::StreamExt as _;
use iroh::address_lookup::{AddrFilter, AddressLookup, EndpointInfo, Item};
use iroh::endpoint::{BeforeConnectOutcome, EndpointHooks};
use iroh::tls::CaTlsConfig;
use iroh::{EndpointId, RelayUrl};
use iroh_relay::server::{
    Access, AccessControl, ClientRateLimit, ClientRequest, ConnectionId, QuicConfig, RelayConfig,
    Server, ServerConfig,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

use crate::sources::{load_trusted_sources, normalize_gateways, TrustedSourcesConfig};

/// Operator override for the pinned relay (tests and staging origins), like
/// `ELASTOS_PUBLISHER_GATEWAY` overrides the stamped publisher at install.
const RELAY_URL_ENV: &str = "ELASTOS_RELAY_URL";

/// Per-Home relay limits. One Home holds one relay connection; a few more
/// cover reconnect overlap. Bytes are what the relay reads from that Home.
const RELAY_CONNECTIONS_PER_HOME: usize = 4;
const RELAY_CONNECTIONS_TOTAL: usize = 4096;
const RELAY_HOME_BYTES_PER_SECOND: u32 = 8 * 1024 * 1024;
const RELAY_HOME_BURST_BYTES: u32 = 16 * 1024 * 1024;

/// The relay this Home accepts, or `None` (direct-only).
pub(crate) fn pinned_relay_url(data_dir: &Path) -> Result<Option<RelayUrl>> {
    let sources = load_trusted_sources(data_dir).context("Cannot read trusted sources")?;
    pinned_relay_url_from(std::env::var(RELAY_URL_ENV).ok().as_deref(), &sources)
}

fn pinned_relay_url_from(
    override_url: Option<&str>,
    sources: &TrustedSourcesConfig,
) -> Result<Option<RelayUrl>> {
    if let Some(raw) = override_url {
        return relay_origin(raw)
            .map(Some)
            .with_context(|| format!("{RELAY_URL_ENV} is not a relay origin"));
    }
    let Some(gateway) = sources
        .default_source()
        .and_then(|source| normalize_gateways(&source.gateways).into_iter().next())
    else {
        return Ok(None);
    };
    match relay_origin(&gateway) {
        Ok(url) => Ok(Some(url)),
        Err(error) => {
            tracing::warn!("carrier: publisher gateway is not a relay origin: {error:#}");
            Ok(None)
        }
    }
}

/// The scheme, host and port of an http(s) URL. Credentials are refused.
fn relay_origin(raw: &str) -> Result<RelayUrl> {
    let url = url::Url::parse(raw.trim()).context("invalid URL")?;
    anyhow::ensure!(
        matches!(url.scheme(), "https" | "http"),
        "relay must use https or http"
    );
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "relay URL must not carry credentials"
    );
    anyhow::ensure!(url.host().is_some(), "relay URL needs a host");
    let origin = url::Url::parse(&url.origin().ascii_serialization())
        .context("relay origin does not parse")?;
    Ok(RelayUrl::from(origin))
}

/// Apply the relay policy to every Carrier endpoint (long-lived and short-lived).
pub(crate) fn apply_relay_policy(
    builder: iroh::endpoint::Builder,
    relay: Option<&RelayUrl>,
) -> iroh::endpoint::Builder {
    let builder = builder
        .addr_filter(AddrFilter::ip_only())
        .hooks(PinnedRelayHook {
            relay: relay.cloned(),
        });
    let Some(relay) = relay else {
        return builder;
    };
    // Address discovery (QUIC on the relay's UDP port) needs the relay's TLS.
    let quic = (relay.scheme() == "https").then(iroh_relay::RelayQuicConfig::default);
    builder
        .relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from_iter([
            iroh::RelayConfig::new(relay.clone(), quic),
        ])))
        .ca_tls_config(pinned_relay_tls(relay))
        .address_lookup(PinnedRelayLookup(relay.clone()))
}

/// The address to dial `peer` with: its IP hints only. Relay addresses in
/// tickets are ignored; the endpoint attaches the pinned relay itself.
pub(crate) fn peer_dial_addr(peer: EndpointId, hints: &[iroh::EndpointAddr]) -> iroh::EndpointAddr {
    iroh::EndpointAddr::from(peer).with_addrs(
        hints
            .iter()
            .filter(|hint| hint.id == peer)
            .flat_map(|hint| hint.addrs.iter())
            .filter(|transport| transport.is_ip())
            .cloned()
            .collect::<Vec<_>>(),
    )
}

/// Published tickets carry IP addresses only, so Homes that refuse every
/// relay (0.8.0-alpha.13 to alpha.15) still dial them directly.
pub(crate) fn published_ticket_addr(addr: iroh::EndpointAddr) -> iroh::EndpointAddr {
    let mut next = addr;
    next.addrs.retain(|transport| transport.is_ip());
    next
}

/// Refuses a dial that names any relay other than the pinned one.
#[derive(Debug)]
struct PinnedRelayHook {
    relay: Option<RelayUrl>,
}

impl EndpointHooks for PinnedRelayHook {
    async fn before_connect<'a>(
        &'a self,
        remote_addr: &'a iroh::EndpointAddr,
        _alpn: &'a [u8],
    ) -> BeforeConnectOutcome {
        if remote_addr
            .relay_urls()
            .all(|url| Some(url) == self.relay.as_ref())
        {
            BeforeConnectOutcome::Accept
        } else {
            BeforeConnectOutcome::Reject
        }
    }
}

/// Every peer is reachable through the pinned relay; iroh asks this lookup
/// whenever it has no working path yet, so a node ID alone dials.
#[derive(Debug, Clone)]
struct PinnedRelayLookup(RelayUrl);

impl AddressLookup for PinnedRelayLookup {
    fn resolve(
        &self,
        endpoint_id: EndpointId,
    ) -> Option<futures_lite::stream::Boxed<Result<Item, iroh::address_lookup::Error>>> {
        let info = EndpointInfo::new(endpoint_id).with_relay_url(self.0.clone());
        Some(futures_lite::stream::once(Ok(Item::new(info, "elastos-relay", None))).boxed())
    }
}

/// TLS to relays (WebSocket and address discovery) verifies the web PKI
/// certificate and also requires the pinned relay's host name, so a relay
/// address learned from a peer cannot pull this Home onto another server.
fn pinned_relay_tls(relay: &RelayUrl) -> CaTlsConfig {
    let host = relay.host_str().unwrap_or_default().to_string();
    CaTlsConfig::custom_server_cert_verifier(Arc::new(move |provider| {
        let host = ServerName::try_from(host.clone())
            .map_err(std::io::Error::other)?
            .to_owned();
        let inner = CaTlsConfig::embedded().server_cert_verifier(provider)?;
        Ok(Arc::new(PinnedRelayHost { host, inner }) as Arc<dyn ServerCertVerifier>)
    }))
}

#[derive(Debug)]
struct PinnedRelayHost {
    host: ServerName<'static>,
    inner: Arc<dyn ServerCertVerifier>,
}

impl ServerCertVerifier for PinnedRelayHost {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if *server_name != self.host {
            return Err(rustls::Error::General(
                "not the pinned ElastOS relay".to_string(),
            ));
        }
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// The seed's relay role from `config.toml`.
///
/// `carrier_relay_bind_addr` serves the relay over plain HTTP for the TLS
/// proxy in front of the public origin (which forwards `/relay` and `/ping`).
/// `carrier_relay_tls_cert` and `carrier_relay_tls_key` (PEM files for the
/// origin's name) add address discovery on UDP 7842, reloaded daily.
/// Only the public publisher role (`gateway_public_publisher_bootstrap`) may
/// run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelayRole {
    bind_addr: SocketAddr,
    tls: Option<(PathBuf, PathBuf)>,
}

pub(crate) fn configured_relay_role(data_dir: &Path) -> Result<Option<RelayRole>> {
    let contents = match std::fs::read_to_string(data_dir.join("config.toml")) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot read Runtime config.toml"),
    };
    relay_role_from(&contents.parse().context("Invalid Runtime config.toml")?)
}

fn relay_role_from(table: &toml::Table) -> Result<Option<RelayRole>> {
    let text = |key: &str| -> Result<Option<&str>> {
        table
            .get(key)
            .map(|value| {
                value
                    .as_str()
                    .with_context(|| format!("{key} must be a string"))
            })
            .transpose()
    };
    let bind = text("carrier_relay_bind_addr")?;
    let cert = text("carrier_relay_tls_cert")?;
    let key = text("carrier_relay_tls_key")?;
    let Some(bind) = bind else {
        anyhow::ensure!(
            cert.is_none() && key.is_none(),
            "carrier_relay_tls_cert and carrier_relay_tls_key need carrier_relay_bind_addr"
        );
        return Ok(None);
    };
    anyhow::ensure!(
        table
            .get("gateway_public_publisher_bootstrap")
            .and_then(toml::Value::as_bool)
            == Some(true),
        "carrier_relay_bind_addr runs only on the public publisher (gateway_public_publisher_bootstrap = true)"
    );
    let bind_addr = bind.parse().context("Invalid carrier_relay_bind_addr")?;
    let tls = match (cert, key) {
        (Some(cert), Some(key)) => Some((PathBuf::from(cert), PathBuf::from(key))),
        (None, None) => None,
        _ => anyhow::bail!("set both carrier_relay_tls_cert and carrier_relay_tls_key, or neither"),
    };
    Ok(Some(RelayRole { bind_addr, tls }))
}

/// The running relay server; dropping it stops the relay.
#[derive(Debug)]
pub(crate) struct RelayServer(Server);

impl RelayServer {
    pub(crate) async fn shutdown(self) {
        if let Err(error) = self.0.shutdown().await {
            tracing::warn!("carrier: relay shutdown: {error:#}");
        }
    }

    #[cfg(test)]
    pub(crate) fn http_addr(&self) -> Option<SocketAddr> {
        self.0.http_addr()
    }

    /// Datagrams the relay forwarded between Homes.
    #[cfg(test)]
    pub(crate) fn relayed_packets(&self) -> u64 {
        self.0.metrics().server.send_packets_sent.get()
    }
}

pub(crate) async fn spawn_relay(role: &RelayRole) -> Result<RelayServer> {
    let mut relay = RelayConfig::new(role.bind_addr);
    let mut rate = ClientRateLimit::new(
        NonZeroU32::new(RELAY_HOME_BYTES_PER_SECOND).expect("nonzero relay rate"),
    );
    rate.max_burst_bytes = NonZeroU32::new(RELAY_HOME_BURST_BYTES);
    relay.limits.client_rx = Some(rate);
    relay.access = Arc::new(RelayAdmission::default());
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    if let Some((cert, key)) = &role.tls {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let resolver = iroh_relay::server::reloading_resolver(
            &provider,
            cert.clone(),
            key.clone(),
            iroh_relay::server::DEFAULT_CERT_RELOAD_INTERVAL,
        )
        .await
        .map_err(|error| anyhow::anyhow!("relay TLS certificate: {error:#}"))?;
        let tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .context("relay TLS 1.3")?
            .with_no_client_auth()
            .with_cert_resolver(resolver);
        let mut quic = QuicConfig::new(SocketAddr::from((
            Ipv6Addr::UNSPECIFIED,
            iroh_relay::defaults::DEFAULT_RELAY_QUIC_PORT,
        )));
        quic.server_config = Some(tls);
        config.quic = Some(quic);
    }
    let server = Server::spawn(config)
        .await
        .map_err(|error| anyhow::anyhow!("Carrier relay did not start: {error:#}"))?;
    tracing::info!(
        "carrier: relay serving on {} (address discovery {})",
        role.bind_addr,
        if role.tls.is_some() { "on" } else { "off" }
    );
    Ok(RelayServer(server))
}

/// Admits a Home while it has fewer than the per-Home connections and the
/// relay is below its total.
#[derive(Debug, Default)]
struct RelayAdmission(std::sync::Mutex<HashMap<EndpointId, usize>>);

impl RelayAdmission {
    fn admit(&self, endpoint_id: EndpointId) -> Access {
        let mut homes = self.0.lock().expect("relay admission lock");
        let total: usize = homes.values().sum();
        let home = homes.entry(endpoint_id).or_default();
        if *home >= RELAY_CONNECTIONS_PER_HOME || total >= RELAY_CONNECTIONS_TOTAL {
            let reason = if *home >= RELAY_CONNECTIONS_PER_HOME {
                "too many relay connections for this Home"
            } else {
                "relay is full"
            };
            if *home == 0 {
                homes.remove(&endpoint_id);
            }
            return Access::Deny {
                reason: Some(reason.to_string()),
            };
        }
        *home += 1;
        Access::Allow
    }

    fn release(&self, endpoint_id: EndpointId) {
        let mut homes = self.0.lock().expect("relay admission lock");
        if let Some(home) = homes.get_mut(&endpoint_id) {
            *home -= 1;
            if *home == 0 {
                homes.remove(&endpoint_id);
            }
        }
    }
}

impl AccessControl for RelayAdmission {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        self.admit(request.endpoint_id())
    }

    fn on_disconnect(&self, endpoint_id: EndpointId, _connection_id: ConnectionId) {
        self.release(endpoint_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::provider::Provider as _;
    use iroh::SecretKey;
    use std::time::Duration;

    fn sources_with_gateway(gateway: &str) -> TrustedSourcesConfig {
        serde_json::from_value(serde_json::json!({
            "schema": "elastos.trusted-sources/v1",
            "default_source": "default",
            "sources": [{"name": "default", "gateways": [gateway]}]
        }))
        .unwrap()
    }

    fn write_sources(data_dir: &Path, gateway: &str) {
        std::fs::create_dir_all(data_dir).unwrap();
        std::fs::write(
            crate::sources::trusted_sources_path(data_dir),
            serde_json::to_vec(&sources_with_gateway(gateway)).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn the_pinned_relay_is_the_publisher_origin_unless_overridden() {
        let none = TrustedSourcesConfig::empty();
        assert_eq!(pinned_relay_url_from(None, &none).unwrap(), None);
        let publisher = sources_with_gateway("https://Elastos.ElacityLabs.com/");
        let pinned = pinned_relay_url_from(None, &publisher).unwrap().unwrap();
        assert_eq!(pinned.as_str(), "https://elastos.elacitylabs.com/");
        let staged = pinned_relay_url_from(Some("http://[::1]:3340"), &publisher)
            .unwrap()
            .unwrap();
        assert_eq!(staged.as_str(), "http://[::1]:3340/");
        // Only the origin pins: path, query and fragment never select a relay.
        assert_eq!(
            relay_origin("https://seed.example/install.sh?x=1#y")
                .unwrap()
                .as_str(),
            "https://seed.example/"
        );
        assert_eq!(
            relay_origin("https:\\\\seed.example\\x")
                .unwrap()
                .host_str(),
            Some("seed.example")
        );
        for refused in [
            "https://user:secret@seed.example",
            "https://user@seed.example",
            "ftp://seed.example",
            "file:///tmp/relay",
            "seed.example",
            "",
        ] {
            assert!(relay_origin(refused).is_err(), "{refused}");
            assert!(
                pinned_relay_url_from(Some(refused), &none).is_err(),
                "{refused}"
            );
        }
        // A gateway that is not a relay origin leaves the Home direct-only.
        let broken = sources_with_gateway("https://user@seed.example");
        assert_eq!(pinned_relay_url_from(None, &broken).unwrap(), None);
    }

    #[test]
    fn relay_tls_accepts_only_the_pinned_host() {
        let relay: RelayUrl = "https://seed.example".parse().unwrap();
        let verifier = pinned_relay_tls(&relay)
            .server_cert_verifier(Arc::new(rustls::crypto::ring::default_provider()))
            .unwrap();
        let junk = CertificateDer::from(vec![0u8; 8]);
        let verify = |host: &str| {
            verifier
                .verify_server_cert(
                    &junk,
                    &[],
                    &ServerName::try_from(host.to_string()).unwrap(),
                    &[],
                    UnixTime::now(),
                )
                .unwrap_err()
                .to_string()
        };
        assert!(verify("relay.other.example").contains("not the pinned ElastOS relay"));
        let pinned = verify("seed.example");
        assert!(
            !pinned.contains("not the pinned ElastOS relay"),
            "the pinned host goes on to web PKI verification: {pinned}"
        );
    }

    #[test]
    fn relay_role_needs_the_public_publisher_and_a_complete_tls_pair() {
        let role = |text: &str| relay_role_from(&text.parse().unwrap());
        assert_eq!(role("dev_mode = true").unwrap(), None);
        let public = "gateway_public_publisher_bootstrap = true\n";
        assert_eq!(
            role(&format!(
                "{public}carrier_relay_bind_addr = \"127.0.0.1:3340\""
            ))
            .unwrap(),
            Some(RelayRole {
                bind_addr: "127.0.0.1:3340".parse().unwrap(),
                tls: None
            })
        );
        let full = role(&format!(
            "{public}carrier_relay_bind_addr = \"127.0.0.1:3340\"\n\
             carrier_relay_tls_cert = \"/tls/fullchain.pem\"\ncarrier_relay_tls_key = \"/tls/privkey.pem\""
        ))
        .unwrap()
        .unwrap();
        assert_eq!(
            full.tls,
            Some(("/tls/fullchain.pem".into(), "/tls/privkey.pem".into()))
        );
        for refused in [
            "carrier_relay_bind_addr = \"127.0.0.1:3340\"".to_string(),
            "gateway_public_publisher_bootstrap = false\ncarrier_relay_bind_addr = \"127.0.0.1:3340\""
                .to_string(),
            format!("{public}carrier_relay_bind_addr = \"localhost\""),
            format!("{public}carrier_relay_bind_addr = 3340"),
            format!(
                "{public}carrier_relay_bind_addr = \"127.0.0.1:3340\"\ncarrier_relay_tls_cert = \"/c\""
            ),
            format!("{public}carrier_relay_tls_key = \"/k\""),
        ] {
            assert!(role(&refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn relay_admission_caps_each_home_and_the_total() {
        let admission = RelayAdmission::default();
        let home = SecretKey::from_bytes(&[7u8; 32]).public();
        for _ in 0..RELAY_CONNECTIONS_PER_HOME {
            assert_eq!(admission.admit(home), Access::Allow);
        }
        assert!(matches!(admission.admit(home), Access::Deny { .. }));
        admission.release(home);
        assert_eq!(admission.admit(home), Access::Allow);

        let others: Vec<EndpointId> = (0..RELAY_CONNECTIONS_TOTAL - RELAY_CONNECTIONS_PER_HOME)
            .map(|index| {
                let mut bytes = [9u8; 32];
                bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
                SecretKey::from_bytes(&bytes).public()
            })
            .collect();
        for other in &others {
            assert_eq!(admission.admit(*other), Access::Allow);
        }
        let late = SecretKey::from_bytes(&[8u8; 32]).public();
        assert!(matches!(admission.admit(late), Access::Deny { .. }));
        assert!(!admission.0.lock().unwrap().contains_key(&late));
        admission.release(others[0]);
        assert_eq!(admission.admit(late), Access::Allow);
    }

    #[tokio::test]
    async fn carrier_endpoints_refuse_every_relay_but_the_pinned_one() {
        let pinned: RelayUrl = "http://127.0.0.1:9".parse().unwrap();
        let foreign: RelayUrl = "https://relay.example.invalid".parse().unwrap();
        let peer = SecretKey::from_bytes(&[62u8; 32]).public();
        let via = |relay: &RelayUrl| {
            iroh::EndpointAddr::from(peer).with_addrs([iroh::TransportAddr::Relay(relay.clone())])
        };
        let pinned_home = super::super::carrier_endpoint_builder(
            SecretKey::from_bytes(&[61u8; 32]),
            Some(&pinned),
        )
        .bind()
        .await
        .unwrap();
        let direct_home =
            super::super::carrier_endpoint_builder(SecretKey::from_bytes(&[63u8; 32]), None)
                .bind()
                .await
                .unwrap();
        for (endpoint, relay) in [
            (&pinned_home, &foreign),
            (&direct_home, &foreign),
            (&direct_home, &pinned),
        ] {
            let error = endpoint
                .connect(via(relay), b"elastos/test")
                .await
                .unwrap_err();
            assert!(
                format!("{error:#}").contains("rejected locally"),
                "{relay}: {error:#}"
            );
        }
        // The pinned relay passes the policy; the dial then waits on that relay.
        match tokio::time::timeout(
            Duration::from_millis(500),
            pinned_home.connect(via(&pinned), b"elastos/test"),
        )
        .await
        {
            Err(_elapsed) => {}
            Ok(result) => {
                let error = result.expect_err("no relay listens on port 9");
                assert!(
                    !format!("{error:#}").contains("rejected locally"),
                    "{error:#}"
                );
            }
        }
        // A node ID alone resolves to the pinned relay.
        let found =
            futures_lite::StreamExt::next(&mut pinned_home.address_lookup().unwrap().resolve(peer))
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        let found = found.into_endpoint_addr();
        assert_eq!(found.relay_urls().collect::<Vec<_>>(), vec![&pinned]);
        pinned_home.close().await;
        direct_home.close().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_node_id_alone_reaches_a_home_through_the_pinned_relay() {
        let relay = spawn_relay(&RelayRole {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            tls: None,
        })
        .await
        .unwrap();
        let relay_url = format!("http://{}", relay.http_addr().unwrap());
        let home = tempfile::tempdir().unwrap();
        write_sources(home.path(), &relay_url);
        let (key, did) = elastos_identity::derive_did(&[65u8; 32]);
        let node = super::super::start_carrier_node_with_registry_bound(
            &key,
            &did,
            home.path().to_path_buf(),
            None,
            Some("127.0.0.1:0".parse().unwrap()),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(10), node.endpoint.online())
            .await
            .expect("the Home reaches its pinned relay");
        let ticket = super::super::CarrierGossipProvider::new(node.gossip_state.clone())
            .send_raw(&serde_json::json!({"op": "get_ticket"}))
            .await
            .unwrap();
        let published =
            super::super::decode_ticket_endpoints(ticket["data"]["ticket"].as_str().unwrap());
        assert!(
            published
                .iter()
                .all(|addr| addr.relay_urls().next().is_none() && !addr.addrs.is_empty()),
            "tickets stay IP-only for Homes that refuse relays: {published:?}"
        );

        // The dialer has no address book and no LAN discovery: only the relay.
        let relay_pin: RelayUrl = relay_url.parse().unwrap();
        let dialer = super::super::carrier_endpoint_builder(
            SecretKey::from_bytes(&[66u8; 32]),
            Some(&relay_pin),
        )
        .bind()
        .await
        .unwrap();
        let client = super::super::CarrierClient::connect_known_endpoint(
            &dialer,
            iroh::EndpointAddr::from(node.endpoint.id()),
            15,
        )
        .await
        .unwrap();
        client.release_head().await.unwrap();
        assert!(
            relay.relayed_packets() > 0,
            "the first packets crossed the relay"
        );
        client.close().await;
        dialer.close().await;
        node.shutdown().await;
        relay.shutdown().await;
    }

    fn nat_env(name: &str) -> String {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} is set by scripts/carrier-nat-test.sh"))
    }

    async fn wait_for_file(path: &Path, deadline: tokio::time::Instant) -> String {
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                return text;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// The seed's relay role in the "internet" namespace of the NAT test.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "run inside network namespaces by scripts/carrier-nat-test.sh"]
    async fn nat_relay() {
        let shared = PathBuf::from(nat_env("ELASTOS_NAT_DIR"));
        let data = shared.join("relay");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            data.join("config.toml"),
            format!(
                "gateway_public_publisher_bootstrap = true\ncarrier_relay_bind_addr = \"{}\"\n",
                nat_env("ELASTOS_NAT_RELAY_BIND")
            ),
        )
        .unwrap();
        let role = configured_relay_role(&data).unwrap().unwrap();
        let relay = spawn_relay(&role).await.unwrap();
        std::fs::write(shared.join("relay.ready"), "").unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
        for home in ["a", "b"] {
            wait_for_file(&shared.join(format!("{home}.done")), deadline).await;
        }
        let forwarded = relay.relayed_packets();
        println!("relay: forwarded {forwarded} datagrams between the Homes");
        assert!(forwarded > 0);
        relay.shutdown().await;
    }

    /// One Home behind its own NAT: dial the other Home by node ID alone, then
    /// exchange Chat messages over Carrier gossip in both directions.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "run inside network namespaces by scripts/carrier-nat-test.sh"]
    async fn nat_home() {
        let shared = PathBuf::from(nat_env("ELASTOS_NAT_DIR"));
        let name = nat_env("ELASTOS_NAT_NAME");
        let peer_name = nat_env("ELASTOS_NAT_PEER");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        let data = shared.join(&name);
        write_sources(&data, &nat_env("ELASTOS_NAT_RELAY_URL"));
        let (key, did) = elastos_identity::derive_did(&[name.as_bytes()[0]; 32]);
        let node = super::super::start_carrier_node_with_registry_bound(
            &key,
            &did,
            data,
            None,
            Some("0.0.0.0:0".parse().unwrap()),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(30), node.endpoint.online())
            .await
            .expect("the Home reaches its pinned relay through its NAT");
        std::fs::write(
            shared.join(format!("{name}.id")),
            node.endpoint.id().to_string(),
        )
        .unwrap();
        let peer: EndpointId = wait_for_file(&shared.join(format!("{peer_name}.id")), deadline)
            .await
            .parse()
            .unwrap();

        let client = super::super::CarrierClient::connect_resolved_peer(&node.endpoint, peer, 60)
            .await
            .unwrap();
        client.release_head().await.unwrap();
        let paths: Vec<String> = client
            .conn
            .paths()
            .iter()
            .map(|path| {
                format!(
                    "{}{}",
                    if path.is_relay() { "relay " } else { "direct " },
                    if path.is_selected() { "(selected)" } else { "" }
                )
            })
            .collect();
        println!("{name}: Carrier connection to {peer_name} by node ID; paths {paths:?}");

        let provider = super::super::CarrierGossipProvider::new(node.gossip_state.clone());
        let topic = "__elastos_test/nat-chat";
        let joined = provider
            .send_raw(&serde_json::json!({
                "op": "gossip_join_exact", "topic": topic, "peers": [peer.to_string()]
            }))
            .await
            .unwrap();
        assert_eq!(joined["status"], "ok", "{joined}");
        let mine = format!("hello from {name}");
        let theirs = format!("hello from {peer_name}");
        let mut nonce = 0u64;
        loop {
            nonce += 1;
            provider
                .send_raw(&serde_json::json!({
                    "op": "gossip_send", "topic": topic, "message": mine,
                    "sender": name, "nonce": nonce
                }))
                .await
                .unwrap();
            let received = provider
                .send_raw(&serde_json::json!({
                    "op": "gossip_recv", "topic": topic, "consumer_id": "nat-test", "limit": 50
                }))
                .await
                .unwrap();
            let got = received["data"]["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|message| message["content"] == theirs.as_str());
            if got && !shared.join(format!("{name}.got")).exists() {
                println!("{name}: received '{theirs}' over Carrier gossip");
                std::fs::write(shared.join(format!("{name}.got")), "").unwrap();
            }
            if shared.join(format!("{name}.got")).exists()
                && shared.join(format!("{peer_name}.got")).exists()
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{name}: no Chat message from {peer_name}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        std::fs::write(shared.join(format!("{name}.done")), "").unwrap();
        wait_for_file(&shared.join(format!("{peer_name}.done")), deadline).await;
        client.close().await;
        node.shutdown().await;
    }
}

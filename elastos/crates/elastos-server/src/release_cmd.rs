use std::sync::Arc;

use elastos_server::sources::TrustedSource;
use elastos_server::update;

pub async fn run_publish_release(
    options: crate::publish::PublishReleaseOptions,
) -> anyhow::Result<()> {
    crate::publish::run_publish_release(options).await
}

pub fn run_source(cmd: crate::sources::SourceCommand) -> anyhow::Result<()> {
    crate::sources::run_source_command(cmd, crate::publish::source_discovery_uri)
}

pub fn run_version(current_version: &str) {
    println!("ElastOS Runtime v{}", current_version);
}

pub async fn run_update_command(
    check: bool,
    head_cid: Option<String>,
    no_p2p: bool,
    gateways: Vec<String>,
    yes: bool,
    rollback_to: Option<String>,
    current_version: &'static str,
) -> anyhow::Result<()> {
    let data_dir = crate::sources::default_data_dir();
    let sources = elastos_server::sources::load_trusted_sources(&data_dir)?;
    let source_config = sources
        .default_source()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("No trusted source configured"))?;
    let carrier_client = if !no_p2p {
        match elastos_server::carrier::CarrierClient::connect_trusted_source(&source_config, 10)
            .await
        {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                anyhow::bail!("Carrier connection failed: {:#}", e);
            }
        }
    } else {
        None
    };

    let platform = update::detect_release_platform().to_string();
    let gateway_only_fetch = no_p2p;
    let fetch_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let carrier_for_fetch = carrier_client.clone();
    let fetch_fn: update::FetchFn = Box::new(move |cid, gateways| {
        let client = carrier_for_fetch.clone();
        let counter = fetch_counter.clone();
        let platform = platform.clone();
        let gateway_only_fetch = gateway_only_fetch;
        Box::pin(async move {
            let mut carrier_error: Option<anyhow::Error> = None;

            if !gateway_only_fetch {
                if let Some(client) = client.as_ref() {
                    let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let path = match n {
                        0 => "release-head.json".to_string(),
                        1 => "release.json".to_string(),
                        2 => format!("elastos-{}", platform),
                        3 => format!("components-{}.json", platform),
                        _ => return Err(anyhow::anyhow!("unexpected fetch #{}", n)),
                    };
                    match client.fetch_file(&path).await {
                        Ok(bytes) => return Ok(bytes),
                        Err(err) => carrier_error = Some(err),
                    }
                } else {
                    carrier_error = Some(anyhow::anyhow!("No Carrier connection"));
                }
            }

            if !gateways.is_empty() {
                return update::fetch_cid_via_gateways(&cid, &gateways).await;
            }

            Err(carrier_error.unwrap_or_else(|| {
                anyhow::anyhow!("No Carrier connection and no gateway configured")
            }))
        })
    });

    let try_p2p: update::TryP2pFn = Box::new(|source, publisher_did| {
        Box::pin(async move { try_p2p_discovery(&source, &publisher_did).await })
    });

    let effective_head_cid = rollback_to.clone().or(head_cid);
    let force = rollback_to.is_some();
    update::run_update(
        &fetch_fn,
        Some(&try_p2p),
        check,
        effective_head_cid,
        no_p2p,
        gateways,
        current_version,
        yes,
        force,
    )
    .await
}

async fn try_p2p_discovery(source: &TrustedSource, _publisher_did: &str) -> Option<String> {
    let client = elastos_server::carrier::CarrierClient::connect_trusted_source(source, 15)
        .await
        .ok()?;
    let release = client.release_head().await.ok()??;
    release["head_cid"].as_str().map(|s| s.to_string())
}

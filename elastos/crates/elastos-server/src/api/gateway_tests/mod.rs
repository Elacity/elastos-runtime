use super::*;
use crate::sources::{save_trusted_sources, TrustedSource, TrustedSourcesConfig};
use axum::body::Body;
use axum::extract::{Path as AxumPath, State as AxumState};
use axum::http::{
    header::{CONTENT_TYPE, COOKIE},
    HeaderMap, Request,
};
use axum::routing::{get, post};
use axum::Json as AxumJson;
use base64::Engine;
use ed25519_dalek::{Signer as _, Verifier as _};
use elastos_runtime::auth::{
    ethereum_signed_message_hash, verify_siwe_challenge, AuthChallengeInput, AuthChallengeV1,
    AuthSessionGrantV1, PasskeyWebAuthnBinding, ProofBinding,
};
use elastos_runtime::provider::{Provider, ProviderError, ResourceRequest, ResourceResponse};
use k256::ecdsa::SigningKey as EvmSigningKey;
use serde_json::json;
use sha2::Sha256;
use sha3::Keccak256;
use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(unix)]
use tokio::io::AsyncReadExt as _;
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex as TokioMutex;
use tower::ServiceExt;

// Real CIDs that pass cid crate validation
const TEST_CIDV0: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
const TEST_CIDV1: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
const GBA_EMULATOR_CAPSULE_ID: &str = "gba-emulator";

fn test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    GatewayState {
        provider_registry: None,
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn documents_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register(Arc::new(crate::documents::DocumentsProvider::new(
            cache_dir.to_path_buf(),
            Arc::downgrade(&registry),
        )))
        .await;
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn library_test_state(cache_dir: &std::path::Path) -> GatewayState {
    library_test_state_with_content(cache_dir, true).await
}

async fn library_test_state_without_content(cache_dir: &std::path::Path) -> GatewayState {
    library_test_state_with_content(cache_dir, false).await
}

async fn library_protected_content_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("content", Arc::new(MockContentProvider))
        .await
        .unwrap();
    registry.register(Arc::new(MockDrmProvider)).await;
    registry.register(Arc::new(MockRightsProvider)).await;
    registry.register(Arc::new(MockKeyProvider)).await;
    registry.register(Arc::new(MockDecryptProvider)).await;
    registry
        .register(Arc::new(crate::library::ObjectProvider::new(
            cache_dir.to_path_buf(),
            Arc::downgrade(&registry),
        )))
        .await;
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn library_external_provider_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("content", Arc::new(MockContentProvider))
        .await
        .unwrap();
    registry
        .register(Arc::new(MockExternalObjectProvider {
            data_dir: cache_dir.to_path_buf(),
        }))
        .await;
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn library_webspace_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register(Arc::new(MockExternalObjectProvider {
            data_dir: cache_dir.to_path_buf(),
        }))
        .await;
    registry
        .register(Arc::new(MockWebSpaceProvider::default()))
        .await;
    registry
        .register(Arc::new(MockWebSpaceAdapterProvider))
        .await;
    registry
        .register(Arc::new(MockOperatorWebSpaceAdapterProvider))
        .await;
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn library_test_state_with_content(
    cache_dir: &std::path::Path,
    with_content: bool,
) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    if with_content {
        registry
            .register_sub_provider("content", Arc::new(MockContentProvider))
            .await
            .unwrap();
    }
    registry
        .register(Arc::new(crate::library::ObjectProvider::new(
            cache_dir.to_path_buf(),
            Arc::downgrade(&registry),
        )))
        .await;
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn chain_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("chain", Arc::new(MockChainProvider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn content_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("content", Arc::new(MockContentProvider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn net_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("net", Arc::new(MockNetProvider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn net_exit_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("net", Arc::new(MockNetProvider))
        .await
        .unwrap();
    registry
        .register_sub_provider("exit", Arc::new(MockExitProvider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn browser_engine_test_state(cache_dir: &std::path::Path) -> GatewayState {
    let state = net_exit_test_state(cache_dir).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider("browser-engine", Arc::new(MockBrowserEngineProvider))
        .await
        .unwrap();
    state
}

async fn browser_engine_attached_test_state(cache_dir: &std::path::Path) -> GatewayState {
    browser_engine_attached_test_state_with_relay(cache_dir, None).await
}

async fn browser_engine_policy_blocked_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("net", Arc::new(MockNetProvider))
        .await
        .unwrap();
    registry
        .register_sub_provider("exit", Arc::new(MockPolicyBlockedExitProvider))
        .await
        .unwrap();
    registry
        .register_sub_provider("browser-engine", Arc::new(MockBrowserEngineProvider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn browser_engine_remote_carrier_exit_test_state(
    cache_dir: &std::path::Path,
) -> GatewayState {
    browser_engine_remote_carrier_exit_test_state_with_close_calls(
        cache_dir,
        Arc::new(TokioMutex::new(Vec::new())),
    )
    .await
}

async fn browser_engine_remote_carrier_exit_test_state_with_close_calls(
    cache_dir: &std::path::Path,
    close_calls: Arc<TokioMutex<Vec<serde_json::Value>>>,
) -> GatewayState {
    browser_engine_remote_carrier_exit_test_state_with_close_failures(cache_dir, close_calls, 0)
        .await
}

async fn browser_engine_remote_carrier_exit_test_state_with_close_failures(
    cache_dir: &std::path::Path,
    close_calls: Arc<TokioMutex<Vec<serde_json::Value>>>,
    close_failures: usize,
) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("net", Arc::new(MockNetProvider))
        .await
        .unwrap();
    registry
        .register_sub_provider(
            "exit",
            Arc::new(MockRemoteCarrierExitProvider::with_close_failures(
                close_calls,
                close_failures,
            )),
        )
        .await
        .unwrap();
    registry
        .register_sub_provider("browser-engine", Arc::new(MockBrowserEngineProvider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn rejecting_browser_engine_remote_carrier_exit_test_state_with_close_calls(
    cache_dir: &std::path::Path,
    close_calls: Arc<TokioMutex<Vec<serde_json::Value>>>,
) -> GatewayState {
    rejecting_browser_engine_remote_carrier_exit_test_state_with_close_failures(
        cache_dir,
        close_calls,
        0,
    )
    .await
}

async fn rejecting_browser_engine_remote_carrier_exit_test_state_with_close_failures(
    cache_dir: &std::path::Path,
    close_calls: Arc<TokioMutex<Vec<serde_json::Value>>>,
    close_failures: usize,
) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("net", Arc::new(MockNetProvider))
        .await
        .unwrap();
    registry
        .register_sub_provider(
            "exit",
            Arc::new(MockRemoteCarrierExitProvider::with_close_failures(
                close_calls,
                close_failures,
            )),
        )
        .await
        .unwrap();
    registry
        .register_sub_provider(
            "browser-engine",
            Arc::new(MockRejectingBrowserEngineProvider),
        )
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn malformed_browser_summary_test_state(cache_dir: &std::path::Path) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("net", Arc::new(MockMalformedNetProvider))
        .await
        .unwrap();
    registry
        .register_sub_provider("exit", Arc::new(MockMalformedExitProvider))
        .await
        .unwrap();
    registry
        .register_sub_provider(
            "browser-engine",
            Arc::new(MockMalformedBrowserEngineProvider),
        )
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn browser_engine_attached_test_state_with_relay(
    cache_dir: &std::path::Path,
    relay_ipc_path: Option<String>,
) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("net", Arc::new(MockNetProvider))
        .await
        .unwrap();
    registry
        .register_sub_provider(
            "exit",
            Arc::new(MockAttachedExitProvider {
                relay_ipc_path,
                stream_id: mock_attached_stream_id(cache_dir),
            }),
        )
        .await
        .unwrap();
    registry
        .register_sub_provider("browser-engine", Arc::new(MockBrowserEngineProvider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn wallet_test_state(cache_dir: &std::path::Path) -> GatewayState {
    wallet_test_state_with_provider(cache_dir, MockWalletProvider::default()).await
}

async fn wallet_test_state_with_provider(
    cache_dir: &std::path::Path,
    provider: MockWalletProvider,
) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("wallet", Arc::new(provider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

async fn wallet_chain_test_state(cache_dir: &std::path::Path) -> GatewayState {
    wallet_chain_test_state_with_wallet_provider(cache_dir, MockWalletProvider::default()).await
}

async fn wallet_chain_test_state_with_wallet_provider(
    cache_dir: &std::path::Path,
    wallet_provider: MockWalletProvider,
) -> GatewayState {
    seed_test_browser_capsules(cache_dir);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("wallet", Arc::new(wallet_provider))
        .await
        .unwrap();
    registry
        .register_sub_provider("chain", Arc::new(MockChainProvider))
        .await
        .unwrap();
    GatewayState {
        provider_registry: Some(registry),
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: cache_dir.to_path_buf(),
        data_dir: cache_dir.to_path_buf(),
    }
}

include!("support_providers.rs");
include!("support_runtime.rs");

mod browser_profile;
mod documents;
#[path = "../gateway_browser_route_tests.rs"]
mod gateway_browser_route_tests;
mod home_system;
mod inspect;
mod library;
mod marketplace;
mod recovery;
mod room;
mod site_publication;
mod wallet;

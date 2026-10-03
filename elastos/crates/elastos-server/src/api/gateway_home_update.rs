//! The existing System owner/passkey surface queues one exact signed update.

use super::*;

const UPDATE_OPERATION: &str = "system.update.apply";
const CHECK_CACHE_SECONDS: u64 = 30;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SystemUpdateApplyRequest {
    action: String,
    request_id: String,
    source_name: String,
    channel: String,
    publisher_did: String,
    current_version: String,
    new_version: String,
    head_cid: String,
    release_cid: String,
    step_up_token: String,
}

impl SystemUpdateApplyRequest {
    fn intent(&self) -> serde_json::Value {
        serde_json::json!({
            "action":self.action,
            "request_id":self.request_id,
            "source_name":self.source_name,
            "channel":self.channel,
            "publisher_did":self.publisher_did,
            "current_version":self.current_version,
            "new_version":self.new_version,
            "head_cid":self.head_cid,
            "release_cid":self.release_cid,
        })
    }

    fn request(&self) -> anyhow::Result<crate::update_controller::UpdateRequest> {
        anyhow::ensure!(self.action == "apply", "unsupported Home update action");
        anyhow::ensure!(
            self.request_id.len() == 32
                && self.request_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid Home update request identity"
        );
        for text in [
            &self.source_name,
            &self.channel,
            &self.publisher_did,
            &self.current_version,
            &self.new_version,
            &self.head_cid,
            &self.release_cid,
        ] {
            anyhow::ensure!(
                !text.is_empty() && text.len() <= 512 && !text.chars().any(char::is_control),
                "invalid Home update choice"
            );
        }
        Ok(crate::update_controller::UpdateRequest {
            id: self.request_id.clone(),
            source_name: self.source_name.clone(),
            channel: self.channel.clone(),
            publisher_did: self.publisher_did.clone(),
            current_version: self.current_version.clone(),
            new_version: self.new_version.clone(),
            head_cid: self.head_cid.clone(),
            release_cid: self.release_cid.clone(),
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct UpdateCheckKey {
    pub(super) data_dir: std::path::PathBuf,
    pub(super) source_policy_sha256: String,
}

struct CachedCheck {
    key: UpdateCheckKey,
    completed_at: std::time::Instant,
    check: Option<crate::operator_control::OperatorUpdateCheck>,
}

struct RunningCheck {
    key: UpdateCheckKey,
    task: tokio::task::JoinHandle<CachedCheck>,
}

pub(super) enum UpdateCheckSnapshot {
    Checking,
    Completed(Option<crate::operator_control::OperatorUpdateCheck>),
}

#[derive(Default)]
pub(super) struct UpdateCheckCache {
    completed: Option<CachedCheck>,
    running: Option<RunningCheck>,
}

impl UpdateCheckCache {
    /// A request only joins a task which has already finished. Policy changes
    /// keep the current worker until it drains, then admit one new worker.
    pub(super) async fn read<F>(
        &mut self,
        key: UpdateCheckKey,
        now: std::time::Instant,
        work: impl FnOnce() -> F,
    ) -> UpdateCheckSnapshot
    where
        F: std::future::Future<Output = Option<crate::operator_control::OperatorUpdateCheck>>
            + Send
            + 'static,
    {
        if self
            .running
            .as_ref()
            .is_some_and(|running| running.task.is_finished())
        {
            let running = self.running.take().expect("finished update check is owned");
            self.completed = Some(running.task.await.unwrap_or_else(|_| CachedCheck {
                key: running.key,
                completed_at: now,
                check: None,
            }));
        }
        if let Some(completed) = self.completed.as_ref().filter(|completed| {
            completed.key == key
                && now.saturating_duration_since(completed.completed_at)
                    < std::time::Duration::from_secs(CHECK_CACHE_SECONDS)
        }) {
            return UpdateCheckSnapshot::Completed(completed.check.clone());
        }
        if self.running.is_none() {
            let task_key = key.clone();
            let check = work();
            self.running = Some(RunningCheck {
                key,
                task: tokio::spawn(async move {
                    let check = check.await;
                    CachedCheck {
                        key: task_key,
                        completed_at: std::time::Instant::now(),
                        check,
                    }
                }),
            });
        }
        UpdateCheckSnapshot::Checking
    }
}

static UPDATE_CHECK: std::sync::OnceLock<tokio::sync::Mutex<UpdateCheckCache>> =
    std::sync::OnceLock::new();

fn source_policy_sha256(config: &crate::sources::TrustedSourcesConfig) -> Option<String> {
    Some(lowercase_hex(&Sha256::digest(
        serde_json::to_vec(config).ok()?,
    )))
}

async fn check_current_policy(
    data_dir: std::path::PathBuf,
    policy_sha256: String,
    source: crate::sources::TrustedSource,
) -> Option<crate::operator_control::OperatorUpdateCheck> {
    let policy_matches = || {
        crate::sources::load_trusted_sources(&data_dir)
            .ok()
            .and_then(|config| source_policy_sha256(&config))
            .is_some_and(|current| current == policy_sha256)
    };
    if !policy_matches() {
        return None;
    }
    let check = crate::operator_control::gather_local_update_check(&data_dir)
        .await
        .ok()?;
    (policy_matches() && check_matches_source(&source, &check)).then_some(check)
}

pub(super) fn check_matches_source(
    source: &crate::sources::TrustedSource,
    check: &crate::operator_control::OperatorUpdateCheck,
) -> bool {
    let channel = if source.channel.trim().is_empty() {
        "stable"
    } else {
        source.channel.trim()
    };
    check.source_name == source.name
        && check.channel == channel
        && check.current_version == source.installed_version
        && source.publisher_dids.contains(&check.publisher_did)
}

pub(super) async fn system_runtime_update_summary(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> Option<serde_json::Value> {
    system_runtime_update_summary_with_cache(
        data_dir,
        context,
        UPDATE_CHECK.get_or_init(|| tokio::sync::Mutex::new(UpdateCheckCache::default())),
        check_current_policy,
    )
    .await
}

pub(super) async fn system_runtime_update_summary_with_cache<F>(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    cache: &tokio::sync::Mutex<UpdateCheckCache>,
    work: impl FnOnce(std::path::PathBuf, String, crate::sources::TrustedSource) -> F,
) -> Option<serde_json::Value>
where
    F: std::future::Future<Output = Option<crate::operator_control::OperatorUpdateCheck>>
        + Send
        + 'static,
{
    let controller = crate::update_controller::status(data_dir).ok()??;
    let config = crate::sources::load_trusted_sources(data_dir).ok()?;
    let source = config.default_source()?;
    if source.installed_version.is_empty() || source.publisher_dids.is_empty() {
        return None;
    }
    let key = UpdateCheckKey {
        data_dir: data_dir.into(),
        source_policy_sha256: source_policy_sha256(&config)?,
    };
    let worker_key = key.clone();
    let worker_source = source.clone();
    let snapshot = cache
        .lock()
        .await
        .read(key, std::time::Instant::now(), || {
            work(
                worker_key.data_dir,
                worker_key.source_policy_sha256,
                worker_source,
            )
        })
        .await;
    let checking = matches!(snapshot, UpdateCheckSnapshot::Checking);
    let check = match &snapshot {
        UpdateCheckSnapshot::Checking => None,
        UpdateCheckSnapshot::Completed(check) => check.as_ref(),
    };
    let available = check.is_some_and(|check| check.update_available);
    let can_apply = available
        && require_admin_principal(data_dir, context).is_ok()
        && check.is_some_and(|check| check.head_cid.is_some() && check.release_cid.is_some())
        && controller.host_pid.is_some()
        && crate::update_controller::has_queued_update(data_dir).is_ok_and(|pending| !pending)
        && matches!(
            controller.phase.as_str(),
            "ready" | "updated" | "restored" | "failed"
        )
        && !crate::install_transaction::InstallTransaction::has_pending_recovery(
            std::path::Path::new(&source.install_path),
        );
    Some(serde_json::json!({
        "configured":true,
        "checking":checking,
        "available":available,
        "can_apply":can_apply,
        "current_version":source.installed_version,
        "new_version":check.map(|check|check.latest_version.as_str()),
        "publisher":check.map(|check|check.publisher_did.as_str()),
        "publisher_did":check.map(|check|check.publisher_did.as_str()),
        "source_name":check.map(|check|check.source_name.as_str()),
        "channel":check.map(|check|check.channel.as_str()),
        "head_cid":check.and_then(|check|check.head_cid.as_deref()),
        "release_cid":check.and_then(|check|check.release_cid.as_deref()),
        "changes":check.map(|check|check.changes.as_slice()).unwrap_or_default(),
        "message":match check {
            None if checking => "Checking for updates.",
            Some(check) if check.update_available => "An update is available.",
            Some(_) => "Home is up to date.",
            None => "Could not check for updates. Check again when Carrier is connected.",
        },
        "controller":controller,
    }))
}

pub(super) async fn system_update_apply(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<SystemUpdateApplyRequest>,
) -> Response {
    let launch =
        match require_home_launch_token_binding(&state.data_dir, &headers, &[SYSTEM_CAPSULE_ID]) {
            Ok(launch) => launch,
            Err(_) => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"error":"Open System from Home and sign in again."})),
                )
                    .into_response()
            }
        };
    if require_admin_principal(&state.data_dir, &launch.context).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error":"Ask the Home owner to install this update."})),
        )
            .into_response();
    }
    let result = system_update_apply_inner(&state, &launch, &input).await;
    match result {
        Ok(response) => Json(response).into_response(),
        Err(_) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error":"The update could not start. Check the update again and approve the current choice."})),
        ).into_response(),
    }
}

async fn system_update_apply_inner(
    state: &GatewayState,
    launch: &RequiredHomeLaunchToken,
    input: &SystemUpdateApplyRequest,
) -> anyhow::Result<serde_json::Value> {
    let request = input.request()?;
    let intent = input.intent();
    let effect = consume_prepared_passkey_step_up_effect(
        &state.data_dir,
        &input.step_up_token,
        launch,
        180,
        UPDATE_OPERATION,
        &intent,
        |effect| {
            crate::update_controller::reserve_owner_update(
                &state.data_dir,
                &request,
                &effect.step_up_id,
                &effect.request_sha256,
                effect.recovered,
            )
        },
    )?;
    if !crate::update_controller::owner_update_is_queued(
        &state.data_dir,
        &request,
        &effect.step_up_id,
        &effect.request_sha256,
    )? {
        let check = crate::operator_control::gather_local_update_check(&state.data_dir).await?;
        anyhow::ensure!(
            choice_matches(&request, &check),
            "Home update choice changed"
        );
    }
    crate::update_controller::queue_owner_update(
        &state.data_dir,
        request.clone(),
        &effect.step_up_id,
        &effect.request_sha256,
    )?;
    Ok(serde_json::json!({
        "id":request.id,
        "phase":"queued",
        "message":"The update is queued. Home will reconnect after Runtime restarts.",
    }))
}

pub(super) fn choice_matches(
    request: &crate::update_controller::UpdateRequest,
    check: &crate::operator_control::OperatorUpdateCheck,
) -> bool {
    check.update_available
        && request.source_name == check.source_name
        && request.channel == check.channel
        && request.publisher_did == check.publisher_did
        && request.current_version == check.current_version
        && request.new_version == check.latest_version
        && Some(request.head_cid.as_str()) == check.head_cid.as_deref()
        && Some(request.release_cid.as_str()) == check.release_cid.as_deref()
}

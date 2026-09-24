//! Reading a GraphQL index of on-chain state, and the consent that allows it.
//!
//! Two surfaces now ask this Home to contact an external index: the Creator's
//! channel picker, and the Marketplace's Shops and Earnings. They ask
//! different questions of the same kind of source, under the same terms --
//! off until the person approves it through the Inbox, the approval recorded
//! with who gave it, every fetch audited, and a failure degrading to a note
//! rather than a refusal.
//!
//! Those terms are written here once. What each surface keeps for itself is
//! its question and its answer's shape; what it borrows is the machinery that
//! decides whether the question may be asked at all.
//!
//! **One endpoint is not one consent.** Each surface carries its own request
//! id, approve action, policy file and audit stream, so approving the channel
//! picker approves the channel picker and nothing else. A descriptor exists
//! precisely so that adding a surface is a new set of those identifiers
//! rather than a second copy of the rules -- the rules drifting apart between
//! copies is the failure this shape is here to prevent.
//!
//! **None of it is authority.** A directory says what to OFFER. The chain
//! decides what is permitted, and every economic act is verified against the
//! chain before it is signed. A directory that is absent, stale, unreachable
//! or wrong costs a convenience, never a capability.

use super::*;

/// One surface's directory: its identity, where its consent is recorded, and
/// the words a person reads when asked for it.
///
/// Every field is `'static`: a directory is a compile-time fact about a
/// surface, not something a request may name. A caller that could choose its
/// own request id or policy path could approve itself.
pub(in crate::api::gateway) struct OnchainDirectory {
    /// The capsule this directory belongs to, for the Inbox item and audits.
    pub capsule_id: &'static str,
    /// How this directory is named in the messages a person or a log reads,
    /// e.g. "channel directory". Lower case; it appears mid-sentence.
    pub subject: &'static str,
    pub request_id: &'static str,
    pub approve_action_id: &'static str,
    pub deny_action_prefix: &'static str,
    /// What follows the deny prefix, naming the source being refused.
    pub deny_action_suffix: &'static str,
    pub source_env: &'static str,
    pub approved_env: &'static str,
    pub policy_schema: &'static str,
    pub policy_root: &'static str,
    pub policy_file: &'static str,
    /// The audit event type without its trailing verb, e.g.
    /// `creator.channel_directory`.
    pub audit_prefix: &'static str,
    pub request_title: &'static str,
    pub request_body: &'static str,
    pub user_agent: &'static str,
}

impl OnchainDirectory {
    /// Whether this directory may be read at all: a source configured, and the
    /// person's approval for it. Checked before any request leaves this Home.
    pub(in crate::api::gateway) fn validate_source(&self, data_dir: &FsPath) -> anyhow::Result<()> {
        self.source_decision(
            |name| std::env::var(name).ok(),
            || self.load_policy(data_dir).ok(),
        )
    }

    /// The decision itself, with its inputs passed in so a test can state a
    /// configuration without an environment or a filesystem.
    pub(in crate::api::gateway) fn source_decision<F, P>(
        &self,
        get_env: F,
        get_policy: P,
    ) -> anyhow::Result<()>
    where
        F: Fn(&str) -> Option<String>,
        P: Fn() -> Option<OnchainDirectoryPolicy>,
    {
        let policy = get_policy();
        let source = get_env(self.source_env)
            .or_else(|| policy.as_ref().map(|policy| policy.source.clone()))
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if source.is_empty() {
            anyhow::bail!(
                "{} source is not configured; approve an external HTTP source to list channels",
                self.subject
            );
        }
        if source != ONCHAIN_DIRECTORY_SOURCE_GRAPHQL {
            anyhow::bail!("unsupported {} source: {source}", self.subject);
        }
        let approved = get_env(self.approved_env)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        // A policy recorded for some other source approves nothing here: the
        // consent named what it was consenting to.
        let policy_approved = policy.as_ref().is_some_and(|policy| {
            policy.external_http_approved && policy.source == ONCHAIN_DIRECTORY_SOURCE_GRAPHQL
        });
        if !matches!(approved.as_str(), "1" | "true" | "yes" | "approved") && !policy_approved {
            anyhow::bail!("external {} HTTP source is not approved", self.subject);
        }
        Ok(())
    }

    /// Whether a failure is one the person can resolve by approving, rather
    /// than one to report. Only the two refusals above mean "ask"; a directory
    /// that is simply down must not put an Inbox item in front of anyone.
    pub(in crate::api::gateway) fn note_should_request_approval(&self, note: &str) -> bool {
        note.contains(&format!("{} source is not configured", self.subject))
            || note.contains(&format!(
                "external {} HTTP source is not approved",
                self.subject
            ))
    }

    /// The index this Home reads.
    ///
    /// It indexes ONE chain, so a Home settling elsewhere must point the
    /// variable at that chain's index. The default is the one this deployment
    /// settles on, so an approved Home gets a list without further
    /// configuration. The wrong index is survivable rather than dangerous --
    /// what it offers is verified on chain before anything is signed -- but it
    /// is a confusing way to fail, so the override is stated rather than
    /// derived from a chain id.
    pub(in crate::api::gateway) fn endpoint(&self) -> String {
        std::env::var(ONCHAIN_GRAPHQL_URL_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| ONCHAIN_GRAPHQL_DEFAULT_URL.to_string())
    }

    /// Asks one question, once. The caller has already established that it may
    /// be asked; this performs no consent check of its own precisely so that
    /// the check cannot be forgotten by being somewhere else.
    pub(in crate::api::gateway) async fn post_graphql(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(ONCHAIN_DIRECTORY_TIMEOUT_SECS))
            .user_agent(self.user_agent)
            .build()?;
        let response = client
            .post(self.endpoint())
            .json(&serde_json::json!({ "query": query, "variables": variables }))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("{} returned {}: {}", self.subject, status, body.trim());
        }
        Ok(response.json::<serde_json::Value>().await?)
    }

    /// The `data.<field>.data` list from a GraphQL answer.
    ///
    /// GraphQL reports failures inside a 200, so `errors` is checked before
    /// `data` -- otherwise a refused query reads as an empty list, and a
    /// surface would quietly show nothing rather than say what happened.
    pub(in crate::api::gateway) fn rows<'a>(
        &self,
        payload: &'a serde_json::Value,
        field: &str,
    ) -> anyhow::Result<&'a Vec<serde_json::Value>> {
        let rows = payload
            .get("data")
            .and_then(|data| data.get(field))
            .and_then(|result| result.get("data"))
            .and_then(serde_json::Value::as_array);
        if let Some(rows) = rows {
            // Rows AND errors together: GraphQL reports a field it could not
            // resolve for one row beside the rows it did resolve. The live
            // index does exactly this -- sixty assets came back with eighteen
            // complaints about a non-nullable `image` that was null -- and
            // treating that as a refusal threw away every good row with the
            // bad ones. A row this Home cannot use is dropped by the caller;
            // a complaint about one row is not an answer about the rest.
            //
            // The complaints are kept, though. Shown to nobody, because they
            // are about the index's own schema rather than about anything a
            // person did, and said out loud in the log, because a shelf
            // quietly missing rows is how this took an afternoon to find.
            if let Some(errors) = payload.get("errors").and_then(serde_json::Value::as_array) {
                if !errors.is_empty() {
                    let first = errors
                        .first()
                        .and_then(|error| error.get("message"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    let paths: Vec<String> = errors
                        .iter()
                        .filter_map(|error| error.get("path"))
                        .take(ONCHAIN_DIRECTORY_LOGGED_ERRORS)
                        .map(|path| path.to_string())
                        .collect();
                    tracing::warn!(
                        subject = self.subject,
                        rows = rows.len(),
                        complaints = errors.len(),
                        first = %first.chars().take(240).collect::<String>(),
                        paths = ?paths,
                        "directory answered with rows and complaints; the rows are used"
                    );
                }
            }
            return Ok(rows);
        }
        if let Some(errors) = payload.get("errors").and_then(serde_json::Value::as_array) {
            let first = errors
                .first()
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("the query was refused");
            anyhow::bail!("{} error: {first}", self.subject);
        }
        anyhow::bail!("{} answer had no {field} list", self.subject)
    }

    pub(in crate::api::gateway) fn policy_path(
        &self,
        data_dir: &FsPath,
    ) -> anyhow::Result<PathBuf> {
        rooted_localhost_fs_path(data_dir, self.policy_root)
            .ok_or_else(|| anyhow::anyhow!("invalid {} policy root", self.subject))
            .map(|root| root.join(self.policy_file))
    }

    pub(in crate::api::gateway) fn load_policy(
        &self,
        data_dir: &FsPath,
    ) -> anyhow::Result<OnchainDirectoryPolicy> {
        let path = self.policy_path(data_dir)?;
        let bytes = std::fs::read(&path)?;
        let policy = serde_json::from_slice::<OnchainDirectoryPolicy>(&bytes)?;
        // A policy file written for another surface is not this surface's
        // consent, whatever it says inside.
        if policy.schema != self.policy_schema {
            anyhow::bail!("unsupported {} policy schema", self.subject);
        }
        Ok(policy)
    }

    pub(in crate::api::gateway) fn store_policy(
        &self,
        data_dir: &FsPath,
        principal_id: &str,
        approved_at: u64,
    ) -> anyhow::Result<()> {
        let policy = OnchainDirectoryPolicy {
            schema: self.policy_schema.to_string(),
            source: ONCHAIN_DIRECTORY_SOURCE_GRAPHQL.to_string(),
            external_http_approved: true,
            approved_by_principal_id: principal_id.to_string(),
            approved_at,
        };
        let path = self.policy_path(data_dir)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&policy)?)?;
        std::fs::rename(temp, path)?;
        Ok(())
    }

    /// Whether an Inbox action belongs to this directory's refusal.
    pub(in crate::api::gateway) fn is_deny_action(&self, action_id: &str) -> bool {
        action_id.strip_prefix(self.deny_action_prefix) == Some(self.deny_action_suffix)
    }

    pub(in crate::api::gateway) fn upsert_request(
        &self,
        data_dir: &FsPath,
        context: &HomeLaunchTokenContext,
        created_at: u64,
    ) -> anyhow::Result<()> {
        crate::notifications::upsert_external_http_request(
            data_dir,
            self.request_id,
            self.capsule_id,
            self.request_title,
            self.request_body,
            self.approve_action_id,
            created_at,
        )?;
        self.append_policy_audit(
            data_dir,
            &context.principal_id,
            &context.session_id,
            "requested",
            "External HTTP access requested",
        )
    }

    pub(in crate::api::gateway) fn append_policy_audit(
        &self,
        data_dir: &FsPath,
        principal_id: &str,
        session_id: &str,
        result: &str,
        reason: &str,
    ) -> anyhow::Result<()> {
        self.append_audit(
            data_dir,
            DirectoryAuditEntry {
                principal_id,
                session_id,
                request_id: self.request_id,
                stream: "policy",
                result,
                reason,
            },
        )
    }

    pub(in crate::api::gateway) fn append_fetch_audit(
        &self,
        data_dir: &FsPath,
        context: &HomeLaunchTokenContext,
        request_id: &str,
        result: &str,
        reason: &str,
    ) -> anyhow::Result<()> {
        self.append_audit(
            data_dir,
            DirectoryAuditEntry {
                principal_id: &context.principal_id,
                session_id: &context.session_id,
                request_id,
                stream: "fetch",
                result,
                reason,
            },
        )
    }

    fn append_audit(
        &self,
        data_dir: &FsPath,
        entry: DirectoryAuditEntry<'_>,
    ) -> anyhow::Result<()> {
        let DirectoryAuditEntry {
            principal_id,
            session_id,
            request_id,
            stream,
            result,
            reason,
        } = entry;
        // The verb is drawn from a fixed set rather than from `result`, so a
        // caller cannot write an event type of its own devising into the audit.
        let verb = match result {
            "requested" => "requested",
            "approved" => "approved",
            "rejected" => "rejected",
            "completed" => "completed",
            "failed" => "failed",
            _ => "other",
        };
        let event_type = format!("{}.{stream}.{verb}", self.audit_prefix);
        append_wallet_approval_audit(
            data_dir,
            WalletApprovalAuditInput {
                capsule_id: self.capsule_id,
                event_type: &event_type,
                principal_id,
                session_id,
                request_id,
                result,
                reason,
            },
        )
    }
}

/// One audit line: who, which request, which stream, and what happened.
struct DirectoryAuditEntry<'a> {
    principal_id: &'a str,
    session_id: &'a str,
    request_id: &'a str,
    /// `policy` for a consent decision, `fetch` for a read performed under it.
    stream: &'a str,
    result: &'a str,
    reason: &'a str,
}

/// One surface's recorded consent. The schema field names which surface, so a
/// file cannot be moved to approve a different one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct OnchainDirectoryPolicy {
    pub schema: String,
    pub source: String,
    pub external_http_approved: bool,
    pub approved_by_principal_id: String,
    pub approved_at: u64,
}

/// The one kind of directory this build knows how to read: a GraphQL index of
/// on-chain state. Names the shape rather than whoever serves it, so an
/// approval recorded today still means the same thing if the host changes.
pub(in crate::api::gateway) const ONCHAIN_DIRECTORY_SOURCE_GRAPHQL: &str = "onchain_graphql";
const ONCHAIN_DIRECTORY_TIMEOUT_SECS: u64 = 6;
/// How many complaint paths one log line carries. Enough to see which field
/// and which rows; not so many that an index having a bad day fills the log.
const ONCHAIN_DIRECTORY_LOGGED_ERRORS: usize = 5;

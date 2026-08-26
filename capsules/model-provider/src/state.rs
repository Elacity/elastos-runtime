use crate::adapters::{
    is_http_job_creating_backend_state, is_local_text_backend_state, AdapterExecutor, CancelResult,
    DispatchResult, ReconcileResult, WorkerApplyGuard,
};
use crate::config::{
    journal_root, BridgeProviderConfig, ConfiguredOffer, ProviderInitExtra,
    MAX_RUN_EVENTS_PAGE_BYTES_LIMIT, MAX_RUN_EVENTS_PAGE_COUNT_LIMIT,
    MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT, MAX_RUN_EVENT_COUNT_LIMIT,
};
use crate::contract::{
    ok_response, validate_run_id, ErrorClass, OfferSummary, OffersListRequest, ProviderFault,
    RunError, RunEvent, RunEventsPage, RunStatus, RunsCancelRequest, RunsCreateRequest,
    RunsEventsRequest, RunsGetRequest, RuntimeAccessBinding, MAX_EVENT_SEQUENCE, PROVIDER_ID,
    PROVIDER_PROTOCOL_VERSION, RUN_EVENTS_SCHEMA, RUN_EVENT_SCHEMA,
};
use crate::journal::validate_run_error;
use crate::journal::{deterministic_run_id, now_ms, request_fingerprint, RunJournal, StoredRun};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub struct ModelProviderState<A: AdapterExecutor> {
    journal: RunJournal,
    offers: BTreeMap<String, ConfiguredOffer>,
    adapters: A,
}

impl<A: AdapterExecutor> ModelProviderState<A> {
    pub fn from_init(config: BridgeProviderConfig, adapters: A) -> Result<Self, ProviderFault> {
        config.validate().map_err(|err| {
            ProviderFault::internal(format!("invalid model provider init config: {err}"))
        })?;
        let extra = serde_json::from_value::<ProviderInitExtra>(config.extra).map_err(|err| {
            ProviderFault::internal(format!("invalid model provider init config: {err}"))
        })?;
        extra.validate(&config.base_path).map_err(|err| {
            ProviderFault::internal(format!("invalid model provider init config: {err}"))
        })?;
        if let Some(provider_id) = extra.provider_id.as_deref() {
            if provider_id != PROVIDER_ID {
                return Err(ProviderFault::internal(format!(
                    "unsupported provider_id: {provider_id}"
                )));
            }
        }
        let journal = RunJournal::open(
            journal_root(&config.base_path, extra.journal_dir.as_deref())
                .map_err(|err| ProviderFault::internal(format!("invalid journal root: {err}")))?,
        )?;
        let mut offers = BTreeMap::new();
        for offer in extra.offers {
            offer
                .validate()
                .map_err(|err| ProviderFault::internal(format!("invalid model offer: {err}")))?;
            if offers.insert(offer.id.clone(), offer).is_some() {
                return Err(ProviderFault::internal(
                    "duplicate model offer id in provider config",
                ));
            }
        }
        Ok(Self {
            journal,
            offers,
            adapters,
        })
    }

    pub fn ready_offer_count(&self) -> usize {
        self.offers.values().filter(|offer| offer.enabled).count()
    }

    pub(crate) fn adapters(&self) -> &A {
        &self.adapters
    }

    pub fn handle_offers_list(&self, request: OffersListRequest) -> Result<Value, ProviderFault> {
        if request.op != "offers_list" {
            return Err(ProviderFault::invalid_request(
                "invalid offers_list request op",
            ));
        }
        let offers = self
            .offers
            .values()
            .filter(|offer| offer.enabled)
            .map(ConfiguredOffer::summary)
            .collect::<Vec<_>>();
        Ok(ok_response(json!({
            "schema": crate::contract::OFFERS_LIST_SCHEMA,
            "provider": PROVIDER_ID,
            "protocol_version": PROVIDER_PROTOCOL_VERSION,
            "offers": offers,
        })))
    }

    pub fn handle_runs_create(
        &mut self,
        request: RunsCreateRequest,
    ) -> Result<Value, ProviderFault> {
        if request.op != "runs_create" {
            return Err(ProviderFault::invalid_request(
                "invalid runs_create request op",
            ));
        }
        reject_untrusted_binding_fields(&request.input)?;
        let offer = self
            .offers
            .get(&request.offer_id)
            .ok_or_else(|| ProviderFault::selection_unavailable("unknown offer_id"))?
            .clone();
        if !offer.enabled {
            return Err(ProviderFault::selection_unavailable("offer is disabled"));
        }
        if request.operation != offer.operation {
            return Err(ProviderFault::invalid_request(
                "requested operation does not match configured offer",
            ));
        }
        request
            .runtime_binding
            .validate(&request.offer_id, &request.operation, &request.input)
            .map_err(|_| ProviderFault::invalid_request("invalid runtime binding"))?;
        let input_bytes = serde_json::to_vec(&request.input)
            .map_err(|err| ProviderFault::internal(format!("failed to encode input: {err}")))?
            .len() as u64;
        if input_bytes > offer.policy.input_bytes_limit {
            return Err(ProviderFault::policy_limit(
                "model input exceeds configured limits",
                format!(
                    "input bytes {} exceed {}",
                    input_bytes, offer.policy.input_bytes_limit
                ),
            ));
        }
        self.journal.prune_expired_terminal_runs()?;
        let fingerprint = request_fingerprint(
            &request.runtime_binding,
            &request.runtime_binding.input_hash,
        )?;
        let run_id = deterministic_run_id(&request.runtime_binding);
        if let Some(mut existing) = self.journal.load_run_if_present(&run_id)? {
            if request_fingerprint(&existing.runtime_binding, &existing.input_hash)? != fingerprint
            {
                return Err(ProviderFault::invalid_request(
                    "request_id conflicts with an existing model run",
                ));
            }
            if !existing.status.is_terminal() {
                let current_offer = self.current_execution_offer_for_run(&existing)?;
                self.refresh_run_if_needed(current_offer.as_ref(), &mut existing)?;
            }
            return serialize_run_view(&existing);
        }

        let created_at_ms = now_ms();
        let offer_summary = offer.summary();
        let mut run = StoredRun::new_prepared(
            run_id,
            request.runtime_binding.clone(),
            offer_summary.clone(),
            offer.execution_binding_hash().map_err(|err| {
                ProviderFault::internal(format!(
                    "failed to derive model execution binding hash: {err}"
                ))
            })?,
            created_at_ms,
        );
        let prepared_offer_id = run.offer.id.clone();
        let prepared_operation = run.offer.operation.clone();
        append_event(
            &offer_summary,
            &mut run,
            "prepared",
            json!({
                "offer_id": prepared_offer_id,
                "operation": prepared_operation,
            }),
            false,
        )?;
        self.journal.store_run(&run)?;

        let active_runs = self.journal.active_run_count(&offer.id)?;
        if active_runs > offer.policy.concurrency_limit as usize {
            transition_error(
                &offer,
                &mut run,
                RunError {
                    class: ErrorClass::SelectionUnavailable,
                    code: "selection_unavailable".to_string(),
                    message: "model offer is not available".to_string(),
                },
            )?;
            self.journal.store_run(&run)?;
            return serialize_run_view(&run);
        }

        match self.adapters.dispatch(
            &offer.adapter,
            &offer,
            &request.runtime_binding,
            &request.input,
        ) {
            Ok(result) => {
                let stored_offer = run.offer.clone();
                apply_dispatch_result(&stored_offer, &mut run, result)?
            }
            Err(fault) => {
                fault.log();
                let stored_offer = run.offer.clone();
                transition_error(&stored_offer, &mut run, fault.error)?;
            }
        }
        self.journal.store_run(&run)?;
        serialize_run_view(&run)
    }

    pub fn handle_runs_get(&mut self, request: RunsGetRequest) -> Result<Value, ProviderFault> {
        if request.op != "runs_get" {
            return Err(ProviderFault::invalid_request(
                "invalid runs_get request op",
            ));
        }
        validate_run_lookup_request(&request.run_id, None, &request.runtime_binding)?;
        let mut run = self.journal.load_run(&request.run_id)?;
        assert_run_owner(&request.runtime_binding, &run)?;
        if self.journal.prune_expired_loaded_run(&run)? {
            return Err(ProviderFault::unauthorized_run_access());
        }
        if !run.status.is_terminal() {
            let offer = self.current_execution_offer_for_run(&run)?;
            self.refresh_run_if_needed(offer.as_ref(), &mut run)?;
        }
        serialize_run_view(&run)
    }

    pub fn handle_runs_events(
        &mut self,
        request: RunsEventsRequest,
    ) -> Result<Value, ProviderFault> {
        if request.op != "runs_events" {
            return Err(ProviderFault::invalid_request(
                "invalid runs_events request op",
            ));
        }
        validate_run_lookup_request(
            &request.run_id,
            request.after_sequence,
            &request.runtime_binding,
        )?;
        let mut run = self.journal.load_run(&request.run_id)?;
        assert_run_owner(&request.runtime_binding, &run)?;
        if self.journal.prune_expired_loaded_run(&run)? {
            return Err(ProviderFault::unauthorized_run_access());
        }
        if !run.status.is_terminal() {
            let offer = self.current_execution_offer_for_run(&run)?;
            self.refresh_run_if_needed(offer.as_ref(), &mut run)?;
        }
        let after_sequence = request.after_sequence.unwrap_or(0);
        Ok(ok_response(
            serde_json::to_value(page_run_events(&run, after_sequence)?).map_err(|err| {
                ProviderFault::internal(format!("failed to serialize run events: {err}"))
            })?,
        ))
    }

    pub fn handle_runs_cancel(
        &mut self,
        request: RunsCancelRequest,
    ) -> Result<Value, ProviderFault> {
        if request.op != "runs_cancel" {
            return Err(ProviderFault::invalid_request(
                "invalid runs_cancel request op",
            ));
        }
        validate_run_lookup_request(&request.run_id, None, &request.runtime_binding)?;
        let mut run = self.journal.load_run(&request.run_id)?;
        assert_run_owner(&request.runtime_binding, &run)?;
        if self.journal.prune_expired_loaded_run(&run)? {
            return Err(ProviderFault::unauthorized_run_access());
        }
        if run.status.is_terminal() {
            return serialize_run_view(&run);
        }
        let offer = self.current_execution_offer_for_run(&run)?;
        self.refresh_run_if_needed(offer.as_ref(), &mut run)?;
        if run.status.is_terminal() {
            return serialize_run_view(&run);
        }
        let offer = offer.ok_or_else(|| {
            ProviderFault::internal("active model run is missing exact configured execution")
        })?;
        let backend_state = run.backend_state.clone().ok_or_else(|| {
            ProviderFault::corrupt_journal("running model run missing backend state")
        })?;
        let reservation = match self.adapters.reserve_cancel(
            &offer.adapter,
            &offer,
            &run.runtime_binding,
            &backend_state,
        ) {
            Ok(reservation) => reservation,
            Err(fault) => {
                fault.log();
                let stored_offer = run.offer.clone();
                transition_error(&stored_offer, &mut run, fault.error)?;
                self.journal.store_run(&run)?;
                return serialize_run_view(&run);
            }
        };
        run.backend_state = Some(reservation.backend_state.clone());
        run.status = RunStatus::Reconciling;
        run.updated_at_ms = now_ms();
        self.journal.store_run(&run)?;
        match self.adapters.cancel(
            &offer.adapter,
            &offer,
            &run.runtime_binding,
            &reservation.backend_state,
            reservation.allow_send,
        ) {
            Ok(result) => {
                let stored_offer = run.offer.clone();
                apply_cancel_result(&stored_offer, &mut run, result)?
            }
            Err(fault) => {
                fault.log();
                let stored_offer = run.offer.clone();
                transition_error(&stored_offer, &mut run, fault.error)?;
            }
        }
        self.journal.store_run(&run)?;
        serialize_run_view(&run)
    }

    pub(crate) fn apply_worker_reconcile_result(
        &mut self,
        run_id: &str,
        guard: WorkerApplyGuard,
        result: ReconcileResult,
    ) -> Result<(), ProviderFault> {
        let Some(mut run) = self.journal.load_run_if_present(run_id)? else {
            return Ok(());
        };
        if run.status.is_terminal() {
            return Ok(());
        }
        let Some(result) = normalize_worker_result(&run, guard, result)? else {
            return Ok(());
        };
        let stored_offer = run.offer.clone();
        apply_reconcile_result(&stored_offer, &mut run, result)?;
        self.journal.store_run(&run)?;
        Ok(())
    }

    pub(crate) fn settle_local_text_run_unknown(
        &mut self,
        run_id: &str,
    ) -> Result<(), ProviderFault> {
        let Some(mut run) = self.journal.load_run_if_present(run_id)? else {
            return Ok(());
        };
        if run.status.is_terminal() {
            return Ok(());
        }
        let is_local = run
            .backend_state
            .as_ref()
            .map(is_local_text_backend_state)
            .unwrap_or(false);
        if !is_local {
            return Ok(());
        }
        let stored_offer = run.offer.clone();
        transition_terminal(
            &stored_offer,
            &mut run,
            RunStatus::SettlementUnknown,
            None,
            Some(RunError {
                class: ErrorClass::SettlementUnknown,
                code: "settlement_unknown".to_string(),
                message: "model backend settlement is unknown".to_string(),
            }),
        )?;
        self.journal.store_run(&run)?;
        Ok(())
    }

    pub(crate) fn settle_active_local_text_runs_unknown(&mut self) -> Result<(), ProviderFault> {
        let run_ids = self
            .journal
            .scan_runs()?
            .into_iter()
            .filter_map(|(_, run)| {
                if run.status.is_terminal() {
                    return None;
                }
                run.backend_state
                    .as_ref()
                    .filter(|backend_state| is_local_text_backend_state(backend_state))
                    .map(|_| run.run_id)
            })
            .collect::<Vec<_>>();
        for run_id in run_ids {
            self.settle_local_text_run_unknown(&run_id)?;
        }
        Ok(())
    }

    pub(crate) fn settle_http_job_create_run_unknown(
        &mut self,
        run_id: &str,
    ) -> Result<(), ProviderFault> {
        let Some(mut run) = self.journal.load_run_if_present(run_id)? else {
            return Ok(());
        };
        if run.status.is_terminal() {
            return Ok(());
        }
        let is_creating = run
            .backend_state
            .as_ref()
            .map(is_http_job_creating_backend_state)
            .unwrap_or(false);
        if !is_creating {
            return Ok(());
        }
        let stored_offer = run.offer.clone();
        transition_terminal(
            &stored_offer,
            &mut run,
            RunStatus::SettlementUnknown,
            None,
            Some(RunError {
                class: ErrorClass::SettlementUnknown,
                code: "settlement_unknown".to_string(),
                message: "model backend settlement is unknown".to_string(),
            }),
        )?;
        self.journal.store_run(&run)?;
        Ok(())
    }

    pub(crate) fn settle_active_http_job_creates_unknown(&mut self) -> Result<(), ProviderFault> {
        let run_ids = self
            .journal
            .scan_runs()?
            .into_iter()
            .filter_map(|(_, run)| {
                if run.status.is_terminal() {
                    return None;
                }
                run.backend_state
                    .as_ref()
                    .filter(|backend_state| is_http_job_creating_backend_state(backend_state))
                    .map(|_| run.run_id)
            })
            .collect::<Vec<_>>();
        for run_id in run_ids {
            self.settle_http_job_create_run_unknown(&run_id)?;
        }
        Ok(())
    }

    fn refresh_run_if_needed(
        &mut self,
        offer: Option<&ConfiguredOffer>,
        run: &mut StoredRun,
    ) -> Result<(), ProviderFault> {
        if run.status == RunStatus::Prepared && run.backend_state.is_none() {
            let stored_offer = run.offer.clone();
            transition_terminal(
                &stored_offer,
                run,
                RunStatus::SettlementUnknown,
                None,
                Some(RunError {
                    class: ErrorClass::SettlementUnknown,
                    code: "settlement_unknown".to_string(),
                    message: "model backend settlement is unknown".to_string(),
                }),
            )?;
            self.journal.store_run(run)?;
            return Ok(());
        }
        if run
            .backend_state
            .as_ref()
            .map(is_http_job_creating_backend_state)
            .unwrap_or(false)
        {
            return Ok(());
        }
        if run.status.is_terminal() {
            if let Some(retention_until_ms) = run.retention_until_ms {
                if now_ms() > retention_until_ms {
                    return Err(ProviderFault::unauthorized_run_access());
                }
            }
            return Ok(());
        }
        let Some(offer) = offer else {
            let stored_offer = run.offer.clone();
            transition_terminal(
                &stored_offer,
                run,
                RunStatus::SettlementUnknown,
                None,
                Some(RunError {
                    class: ErrorClass::SettlementUnknown,
                    code: "settlement_unknown".to_string(),
                    message: "model backend settlement is unknown".to_string(),
                }),
            )?;
            self.journal.store_run(run)?;
            return Ok(());
        };
        if now_ms() > run.deadline_ms {
            let stored_offer = run.offer.clone();
            transition_terminal(
                &stored_offer,
                run,
                RunStatus::Failed,
                None,
                Some(RunError {
                    class: ErrorClass::BackendTimeout,
                    code: "backend_timeout".to_string(),
                    message: "model backend timed out".to_string(),
                }),
            )?;
            self.journal.store_run(run)?;
            return Ok(());
        }
        let Some(backend_state) = run.backend_state.as_ref() else {
            return Ok(());
        };
        match self
            .adapters
            .reconcile(&offer.adapter, offer, &run.runtime_binding, backend_state)
        {
            Ok(result) => {
                let stored_offer = run.offer.clone();
                apply_reconcile_result(&stored_offer, run, result)?
            }
            Err(fault) => {
                fault.log();
                let stored_offer = run.offer.clone();
                transition_error(&stored_offer, run, fault.error)?;
            }
        }
        self.journal.store_run(run)?;
        Ok(())
    }

    fn current_execution_offer_for_run(
        &self,
        run: &StoredRun,
    ) -> Result<Option<ConfiguredOffer>, ProviderFault> {
        let Some(offer) = self.offers.get(&run.offer.id) else {
            return Ok(None);
        };
        let current_summary = offer.summary();
        let current_hash = offer.execution_binding_hash().map_err(|err| {
            ProviderFault::internal(format!(
                "failed to derive model execution binding hash: {err}"
            ))
        })?;
        if current_summary != run.offer || current_hash != run.execution_binding_hash {
            return Ok(None);
        }
        Ok(Some(offer.clone()))
    }
}

fn normalize_worker_result(
    run: &StoredRun,
    guard: WorkerApplyGuard,
    result: ReconcileResult,
) -> Result<Option<ReconcileResult>, ProviderFault> {
    match guard {
        WorkerApplyGuard::None => Ok(Some(result)),
        WorkerApplyGuard::HttpArtifactBackendState { backend_state } => {
            if run.backend_state.as_ref() != Some(&backend_state) {
                return Ok(None);
            }
            Ok(Some(result))
        }
    }
}

fn reject_untrusted_binding_fields(input: &Value) -> Result<(), ProviderFault> {
    reject_untrusted_binding_fields_at(input)
}

fn assert_run_owner(binding: &RuntimeAccessBinding, run: &StoredRun) -> Result<(), ProviderFault> {
    binding
        .validate(&run.run_id)
        .map_err(|_| ProviderFault::invalid_request("invalid runtime access binding"))?;
    if run.runtime_binding.principal_id != binding.principal_id
        || run.runtime_binding.capsule_id != binding.capsule_id
    {
        return Err(ProviderFault::unauthorized_run_access());
    }
    Ok(())
}

trait OfferLimitsView {
    fn event_bytes_limit(&self) -> u64;
    fn retention_secs(&self) -> u64;
    fn inline_output_bytes_limit(&self) -> u64;
}

impl OfferLimitsView for OfferSummary {
    fn event_bytes_limit(&self) -> u64 {
        self.policy.event_bytes_limit
    }

    fn retention_secs(&self) -> u64 {
        self.policy.retention_secs
    }

    fn inline_output_bytes_limit(&self) -> u64 {
        self.policy.inline_output_bytes_limit
    }
}

impl OfferLimitsView for ConfiguredOffer {
    fn event_bytes_limit(&self) -> u64 {
        self.policy.event_bytes_limit
    }

    fn retention_secs(&self) -> u64 {
        self.policy.retention_secs
    }

    fn inline_output_bytes_limit(&self) -> u64 {
        self.policy.inline_output_bytes_limit
    }
}

fn append_event(
    offer: &impl OfferLimitsView,
    run: &mut StoredRun,
    kind: impl Into<String>,
    data: Value,
    terminal: bool,
) -> Result<(), ProviderFault> {
    if run.next_sequence > MAX_EVENT_SEQUENCE {
        return Err(ProviderFault::internal(
            "model run event cursor exceeded provider limit",
        ));
    }
    let event = RunEvent {
        schema: RUN_EVENT_SCHEMA.to_string(),
        sequence: run.next_sequence,
        kind: kind.into(),
        data,
        terminal,
    };
    let encoded = serde_json::to_vec(&event)
        .map_err(|err| ProviderFault::internal(format!("failed to encode run event: {err}")))?;
    if encoded.len() as u64 > offer.event_bytes_limit() {
        return Err(ProviderFault::policy_limit(
            "model event exceeds configured limits",
            format!(
                "event bytes {} exceed {}",
                encoded.len(),
                offer.event_bytes_limit()
            ),
        ));
    }
    let count_limit = if terminal {
        MAX_RUN_EVENT_COUNT_LIMIT
    } else {
        MAX_RUN_EVENT_COUNT_LIMIT.saturating_sub(1)
    };
    if run.events.len() >= count_limit {
        return Err(ProviderFault::policy_limit(
            "model run event retention exceeds provider limits",
            format!(
                "event count {} exceeds {}",
                run.events.len() + 1,
                count_limit
            ),
        ));
    }
    let aggregate_limit = if terminal {
        MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT
    } else {
        MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT.saturating_sub(crate::config::MAX_EVENT_BYTES_LIMIT)
    };
    let aggregate_bytes = run_event_bytes_total(&run.events)?.saturating_add(encoded.len() as u64);
    if aggregate_bytes > aggregate_limit {
        return Err(ProviderFault::policy_limit(
            "model run event retention exceeds provider limits",
            format!(
                "aggregate event bytes {} exceed {}",
                aggregate_bytes, aggregate_limit
            ),
        ));
    }
    run.events.push(event);
    run.next_sequence = run.next_sequence.saturating_add(1);
    run.updated_at_ms = now_ms();
    Ok(())
}

fn append_adapter_event_seed(
    offer: &impl OfferLimitsView,
    run: &mut StoredRun,
    event: crate::adapters::EventSeed,
) -> Result<(), ProviderFault> {
    if matches!(
        event.kind,
        "output" | "failed" | "cancelled" | "settlement_unknown"
    ) {
        return Err(ProviderFault::internal(
            "adapter event seeds must not use terminal event kinds",
        ));
    }
    append_event(offer, run, event.kind, event.data, false)
}

fn page_run_events(run: &StoredRun, after_sequence: u64) -> Result<RunEventsPage, ProviderFault> {
    let current_cursor = run.next_sequence.saturating_sub(1);
    if after_sequence >= current_cursor {
        return Ok(RunEventsPage {
            schema: RUN_EVENTS_SCHEMA.to_string(),
            run_id: run.run_id.clone(),
            next_cursor: current_cursor,
            has_more: false,
            events: Vec::new(),
        });
    }

    let mut events = Vec::new();
    let mut total_bytes = 0u64;
    let mut next_cursor = after_sequence;
    let mut has_more = false;

    for event in run
        .events
        .iter()
        .filter(|event| event.sequence > after_sequence)
    {
        let encoded_len = encoded_run_event_len(event)?;
        if !events.is_empty()
            && (events.len() >= MAX_RUN_EVENTS_PAGE_COUNT_LIMIT
                || total_bytes.saturating_add(encoded_len) > MAX_RUN_EVENTS_PAGE_BYTES_LIMIT)
        {
            has_more = true;
            break;
        }
        if encoded_len > MAX_RUN_EVENTS_PAGE_BYTES_LIMIT {
            return Err(ProviderFault::internal(
                "stored model event exceeds provider page limit",
            ));
        }
        total_bytes = total_bytes.saturating_add(encoded_len);
        next_cursor = event.sequence;
        events.push(event.clone());
    }

    Ok(RunEventsPage {
        schema: RUN_EVENTS_SCHEMA.to_string(),
        run_id: run.run_id.clone(),
        next_cursor,
        has_more,
        events,
    })
}

fn transition_terminal(
    offer: &impl OfferLimitsView,
    run: &mut StoredRun,
    status: RunStatus,
    output: Option<Value>,
    error: Option<RunError>,
) -> Result<(), ProviderFault> {
    if run.status.is_terminal() {
        return Err(ProviderFault::internal(
            "model run attempted a second terminal transition",
        ));
    }
    let (output, error) = validate_terminal_payload(offer, &status, output, error)?;
    let (terminal_kind, terminal_data) =
        synthesize_terminal_event(status.clone(), output.as_ref(), error.as_ref())?;
    append_event(offer, run, &terminal_kind, terminal_data, true)?;
    if run.events.last().map(|event| event.terminal) != Some(true) {
        return Err(ProviderFault::internal(
            "model run terminal transition missing terminal event",
        ));
    }
    match status {
        RunStatus::Completed => {
            run.output = output;
            run.error = None;
        }
        RunStatus::Cancelled | RunStatus::Failed | RunStatus::SettlementUnknown => {
            run.output = None;
            run.error = error;
        }
        _ => {
            return Err(ProviderFault::internal(
                "invalid non-terminal status in terminal transition",
            ))
        }
    }
    run.status = status;
    run.backend_state = None;
    run.terminal_at_ms = Some(now_ms());
    run.retention_until_ms = run
        .terminal_at_ms
        .map(|at| at.saturating_add(offer.retention_secs().saturating_mul(1_000)));
    run.updated_at_ms = now_ms();
    Ok(())
}

fn transition_error(
    offer: &impl OfferLimitsView,
    run: &mut StoredRun,
    error: RunError,
) -> Result<(), ProviderFault> {
    let status = match error.class {
        ErrorClass::Cancelled => RunStatus::Cancelled,
        ErrorClass::SettlementUnknown => RunStatus::SettlementUnknown,
        _ => RunStatus::Failed,
    };
    transition_terminal(offer, run, status, None, Some(error))
}

fn apply_run_atomically(
    run: &mut StoredRun,
    apply: impl FnOnce(&mut StoredRun) -> Result<(), ProviderFault>,
) -> Result<(), ProviderFault> {
    let mut staged = run.clone();
    apply(&mut staged)?;
    *run = staged;
    Ok(())
}

fn apply_dispatch_result(
    offer: &impl OfferLimitsView,
    run: &mut StoredRun,
    result: DispatchResult,
) -> Result<(), ProviderFault> {
    apply_run_atomically(run, |staged| {
        match result {
            DispatchResult::Terminal {
                events,
                status,
                output,
                error,
            } => {
                collect_non_terminal_events(offer, staged, events)?;
                transition_terminal(offer, staged, status, output, error)?;
            }
            DispatchResult::Running {
                events,
                backend_state,
            } => {
                for event in events {
                    append_adapter_event_seed(offer, staged, event)?;
                }
                staged.backend_state = Some(backend_state);
                staged.status = RunStatus::Running;
                staged.updated_at_ms = now_ms();
            }
        }
        Ok(())
    })
}

fn apply_reconcile_result(
    offer: &impl OfferLimitsView,
    run: &mut StoredRun,
    result: ReconcileResult,
) -> Result<(), ProviderFault> {
    apply_run_atomically(run, |staged| {
        match result {
            ReconcileResult::StillRunning {
                events,
                backend_state,
                status,
            } => {
                for event in events {
                    append_adapter_event_seed(offer, staged, event)?;
                }
                staged.backend_state = Some(backend_state);
                staged.status = status;
                staged.updated_at_ms = now_ms();
            }
            ReconcileResult::Terminal {
                events,
                status,
                output,
                error,
            } => {
                collect_non_terminal_events(offer, staged, events)?;
                transition_terminal(offer, staged, status, output, error)?;
            }
        }
        Ok(())
    })
}

fn apply_cancel_result(
    offer: &impl OfferLimitsView,
    run: &mut StoredRun,
    result: CancelResult,
) -> Result<(), ProviderFault> {
    apply_run_atomically(run, |staged| {
        match result {
            CancelResult::Reconciling {
                events,
                backend_state,
            } => {
                collect_non_terminal_events(offer, staged, events)?;
                staged.backend_state = Some(backend_state);
                staged.status = RunStatus::Reconciling;
                staged.updated_at_ms = now_ms();
            }
            CancelResult::SettlementUnknown { events } => {
                collect_non_terminal_events(offer, staged, events)?;
                transition_terminal(
                    offer,
                    staged,
                    RunStatus::SettlementUnknown,
                    None,
                    Some(RunError {
                        class: ErrorClass::SettlementUnknown,
                        code: "settlement_unknown".to_string(),
                        message: "model backend settlement is unknown".to_string(),
                    }),
                )?;
            }
            CancelResult::Terminal {
                events,
                status,
                output,
                error,
            } => {
                collect_non_terminal_events(offer, staged, events)?;
                transition_terminal(offer, staged, status, output, error)?;
            }
        }
        Ok(())
    })
}

fn collect_non_terminal_events(
    offer: &impl OfferLimitsView,
    run: &mut StoredRun,
    events: Vec<crate::adapters::EventSeed>,
) -> Result<(), ProviderFault> {
    for event in events {
        append_adapter_event_seed(offer, run, event)?;
    }
    Ok(())
}

fn synthesize_terminal_event(
    status: RunStatus,
    output: Option<&Value>,
    error: Option<&RunError>,
) -> Result<(String, Value), ProviderFault> {
    match status {
        RunStatus::Completed => Ok((
            "output".to_string(),
            output
                .cloned()
                .ok_or_else(|| ProviderFault::internal("completed model run output is missing"))?,
        )),
        RunStatus::Cancelled => {
            let error = error
                .ok_or_else(|| ProviderFault::internal("cancelled model run error is missing"))?;
            Ok((
                "cancelled".to_string(),
                json!({
                    "class": "cancelled",
                    "code": error.code,
                    "message": error.message,
                }),
            ))
        }
        RunStatus::SettlementUnknown => {
            let error = error.ok_or_else(|| {
                ProviderFault::internal("settlement_unknown model run error is missing")
            })?;
            Ok((
                "settlement_unknown".to_string(),
                json!({
                    "class": "settlement_unknown",
                    "code": error.code,
                    "message": error.message,
                }),
            ))
        }
        RunStatus::Failed => {
            let error = error
                .ok_or_else(|| ProviderFault::internal("failed model run error is missing"))?;
            Ok((
                "failed".to_string(),
                json!({
                    "class": serde_json::to_value(&error.class).map_err(|err| {
                        ProviderFault::internal(format!(
                            "failed to serialize model run error class: {err}"
                        ))
                    })?,
                    "code": error.code,
                    "message": error.message,
                }),
            ))
        }
        _ => Err(ProviderFault::internal(
            "invalid non-terminal status in terminal transition",
        )),
    }
}

fn validate_run_lookup_request(
    run_id: &str,
    after_sequence: Option<u64>,
    binding: &RuntimeAccessBinding,
) -> Result<(), ProviderFault> {
    validate_run_id(run_id).map_err(|_| ProviderFault::invalid_request("invalid run_id"))?;
    binding
        .validate(run_id)
        .map_err(|_| ProviderFault::invalid_request("invalid runtime access binding"))?;
    if let Some(after_sequence) = after_sequence {
        if after_sequence > MAX_EVENT_SEQUENCE {
            return Err(ProviderFault::invalid_request("invalid model event cursor"));
        }
    }
    Ok(())
}

fn serialize_run_view(run: &StoredRun) -> Result<Value, ProviderFault> {
    Ok(ok_response(serde_json::to_value(run.to_view()).map_err(
        |err| ProviderFault::internal(format!("failed to serialize run view: {err}")),
    )?))
}

fn validate_completed_output(
    offer: &impl OfferLimitsView,
    status: &RunStatus,
    output: Option<Value>,
) -> Result<Value, ProviderFault> {
    if *status != RunStatus::Completed {
        return Err(ProviderFault::internal(
            "completed output validation requires completed status",
        ));
    }
    let Some(output) = output else {
        return Err(ProviderFault::internal(
            "completed model run output is missing",
        ));
    };
    let encoded = serde_json::to_vec(&output)
        .map_err(|err| ProviderFault::internal(format!("failed to encode output: {err}")))?;
    if encoded.len() as u64 > offer.inline_output_bytes_limit() {
        return Err(ProviderFault::policy_limit(
            "model output exceeds configured limits",
            format!(
                "output bytes {} exceed {}",
                encoded.len(),
                offer.inline_output_bytes_limit()
            ),
        ));
    }
    Ok(output)
}

fn validate_terminal_payload(
    offer: &impl OfferLimitsView,
    status: &RunStatus,
    output: Option<Value>,
    error: Option<RunError>,
) -> Result<(Option<Value>, Option<RunError>), ProviderFault> {
    match status {
        RunStatus::Completed => {
            if error.is_some() {
                return Err(ProviderFault::internal(
                    "completed model run must not include a terminal error",
                ));
            }
            Ok((
                Some(validate_completed_output(offer, status, output)?),
                None,
            ))
        }
        RunStatus::Failed | RunStatus::Cancelled | RunStatus::SettlementUnknown => {
            if output.is_some() {
                return Err(ProviderFault::internal(
                    "terminal model run error states must not include output",
                ));
            }
            let error = error
                .ok_or_else(|| ProviderFault::internal("terminal model run error is missing"))?;
            validate_run_error(&error)
                .map_err(|_| ProviderFault::internal("terminal model run error is invalid"))?;
            Ok((None, Some(error)))
        }
        _ => Err(ProviderFault::internal(
            "invalid non-terminal status in terminal transition",
        )),
    }
}

fn encoded_run_event_len(event: &RunEvent) -> Result<u64, ProviderFault> {
    Ok(serde_json::to_vec(event)
        .map_err(|err| ProviderFault::internal(format!("failed to encode run event: {err}")))?
        .len() as u64)
}

fn run_event_bytes_total(events: &[RunEvent]) -> Result<u64, ProviderFault> {
    events.iter().try_fold(0u64, |total, event| {
        encoded_run_event_len(event).map(|bytes| total.saturating_add(bytes))
    })
}

fn reject_untrusted_binding_fields_at(input: &Value) -> Result<(), ProviderFault> {
    const FORBIDDEN_KEYS: &[&str] = &[
        "principal_id",
        "session_id",
        "capsule_id",
        "grant_id",
        "request_id",
        "offer_id",
        "operation",
        "input_hash",
        "backend_url",
        "process_path",
        "carrier_route",
        "endpoint_did",
        "port",
    ];
    match input {
        Value::Object(object) => {
            for (key, value) in object {
                if FORBIDDEN_KEYS.contains(&key.as_str()) {
                    return Err(ProviderFault::invalid_request(
                        "caller input must not declare runtime binding fields",
                    ));
                }
                reject_untrusted_binding_fields_at(value)?;
            }
            Ok(())
        }
        Value::Array(values) => {
            for value in values {
                reject_untrusted_binding_fields_at(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{
        serialize_local_text_backend_state, AdapterFault, CancelReservation, EventSeed,
    };
    use crate::config::{
        AdapterConfig, OfferPolicy, MAX_EVENT_BYTES_LIMIT, MAX_INLINE_OUTPUT_BYTES_LIMIT,
        MAX_RUN_EVENTS_PAGE_COUNT_LIMIT, MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT,
        MAX_RUN_EVENT_COUNT_LIMIT,
    };
    use crate::contract::{
        model_input_hash, RuntimeCreateBinding, RUNTIME_ACCESS_BINDING_SCHEMA,
        RUNTIME_CREATE_BINDING_SCHEMA, RUN_OUTPUT_OBJECT_SCHEMA, RUN_OUTPUT_TEXT_SCHEMA,
    };
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakeAdapters {
        dispatch_results: Arc<Mutex<Vec<std::result::Result<DispatchResult, AdapterFault>>>>,
        reconcile_results: Arc<Mutex<Vec<std::result::Result<ReconcileResult, AdapterFault>>>>,
        reserve_cancel_results:
            Arc<Mutex<Vec<std::result::Result<CancelReservation, AdapterFault>>>>,
        cancel_results: Arc<Mutex<Vec<std::result::Result<CancelResult, AdapterFault>>>>,
        cancel_allow_send: Arc<Mutex<Vec<bool>>>,
        dispatch_calls: Arc<Mutex<u32>>,
        reconcile_calls: Arc<Mutex<u32>>,
    }

    impl AdapterExecutor for FakeAdapters {
        fn dispatch(
            &self,
            _adapter: &AdapterConfig,
            _offer: &ConfiguredOffer,
            _binding: &RuntimeCreateBinding,
            _input: &Value,
        ) -> std::result::Result<DispatchResult, AdapterFault> {
            *self.dispatch_calls.lock().unwrap() += 1;
            self.dispatch_results.lock().unwrap().remove(0)
        }

        fn reconcile(
            &self,
            _adapter: &AdapterConfig,
            _offer: &ConfiguredOffer,
            _binding: &RuntimeCreateBinding,
            _backend_state: &Value,
        ) -> std::result::Result<ReconcileResult, AdapterFault> {
            *self.reconcile_calls.lock().unwrap() += 1;
            self.reconcile_results.lock().unwrap().remove(0)
        }

        fn cancel(
            &self,
            _adapter: &AdapterConfig,
            _offer: &ConfiguredOffer,
            _binding: &RuntimeCreateBinding,
            _backend_state: &Value,
            allow_send: bool,
        ) -> std::result::Result<CancelResult, AdapterFault> {
            self.cancel_allow_send.lock().unwrap().push(allow_send);
            self.cancel_results.lock().unwrap().remove(0)
        }

        fn reserve_cancel(
            &self,
            _adapter: &AdapterConfig,
            _offer: &ConfiguredOffer,
            _binding: &RuntimeCreateBinding,
            _backend_state: &Value,
        ) -> std::result::Result<CancelReservation, AdapterFault> {
            self.reserve_cancel_results.lock().unwrap().remove(0)
        }
    }

    fn temp_root(label: &str) -> String {
        crate::test_support::temp_root_path("model-provider-state", label)
            .to_string_lossy()
            .to_string()
    }

    fn offer(id: &str) -> ConfiguredOffer {
        ConfiguredOffer {
            id: id.to_string(),
            title: format!("Offer {id}"),
            operation: "text.generate".to_string(),
            input_modalities: vec!["text/plain".to_string()],
            output_modalities: vec!["text/plain".to_string()],
            policy: OfferPolicy {
                concurrency_limit: 1,
                input_bytes_limit: 8 * 1024,
                inline_output_bytes_limit: 8 * 1024,
                event_bytes_limit: 8 * 1024,
                runtime_ms_limit: 30_000,
                retention_secs: 60,
                cancel_settlement_timeout_ms: 5,
            },
            adapter: AdapterConfig::OpenAiCompatibleText {
                api_url: "https://example.invalid/v1/chat/completions".to_string(),
                api_key: None,
                model: "gpt-test".to_string(),
            },
            enabled: true,
        }
    }

    fn create_binding(request_id: &str, offer_id: &str, input: &Value) -> RuntimeCreateBinding {
        RuntimeCreateBinding {
            schema: RUNTIME_CREATE_BINDING_SCHEMA.to_string(),
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            capsule_id: "assistant".to_string(),
            grant_id: "grant:test".to_string(),
            request_id: request_id.to_string(),
            offer_id: offer_id.to_string(),
            operation: "text.generate".to_string(),
            input_hash: model_input_hash(input).unwrap(),
        }
    }

    fn access_binding(binding: &RuntimeCreateBinding) -> RuntimeAccessBinding {
        let run_id = deterministic_run_id(binding);
        RuntimeAccessBinding {
            schema: RUNTIME_ACCESS_BINDING_SCHEMA.to_string(),
            principal_id: binding.principal_id.clone(),
            session_id: binding.session_id.clone(),
            capsule_id: binding.capsule_id.clone(),
            grant_id: binding.grant_id.clone(),
            request_id: binding.request_id.clone(),
            run_id,
        }
    }

    fn rotate_create_audit(
        binding: &RuntimeCreateBinding,
        session_id: &str,
        grant_id: &str,
    ) -> RuntimeCreateBinding {
        let mut rotated = binding.clone();
        rotated.session_id = session_id.to_string();
        rotated.grant_id = grant_id.to_string();
        rotated
    }

    fn rotate_access_invocation(
        binding: &RuntimeCreateBinding,
        session_id: &str,
        grant_id: &str,
        request_id: &str,
    ) -> RuntimeAccessBinding {
        let mut rotated = access_binding(binding);
        rotated.session_id = session_id.to_string();
        rotated.grant_id = grant_id.to_string();
        rotated.request_id = request_id.to_string();
        rotated
    }

    fn run_path(root: &str, run_id: &str) -> std::path::PathBuf {
        let journal_root = crate::config::journal_root(root, None).unwrap();
        crate::journal::hashed_path(journal_root.join("runs").as_path(), run_id)
    }

    fn expire_terminal_run(run: &mut StoredRun) {
        let retention_ms = run.offer.policy.retention_secs.saturating_mul(1_000);
        let expired_terminal_at_ms = now_ms().saturating_sub(retention_ms.saturating_add(1_000));
        run.created_at_ms =
            expired_terminal_at_ms.saturating_sub(run.offer.policy.runtime_ms_limit);
        run.deadline_ms = run
            .created_at_ms
            .saturating_add(run.offer.policy.runtime_ms_limit);
        run.terminal_at_ms = Some(expired_terminal_at_ms);
        run.updated_at_ms = run.updated_at_ms.max(expired_terminal_at_ms);
        run.retention_until_ms = Some(expired_terminal_at_ms.saturating_add(retention_ms));
    }

    fn reserved_cancel_backend_state(deadline_ms: u64, cancel_sent: bool) -> Value {
        serde_json::json!({
            "schema": "elastos.model.provider-http-job-state/v1",
            "job_id": "job-1",
            "next_poll_at_ms": 0,
            "cancel_requested": true,
            "cancel_sent": cancel_sent,
            "cancel_deadline_ms": deadline_ms,
        })
    }

    fn running_backend_state() -> Value {
        serde_json::json!({
            "schema": "elastos.model.provider-http-job-state/v1",
            "job_id": "job-1",
            "next_poll_at_ms": u64::MAX,
            "cancel_requested": false,
            "cancel_sent": false,
            "cancel_deadline_ms": null,
        })
    }

    fn event_data_bytes(bytes: usize) -> Value {
        serde_json::json!({
            "text": "x".repeat(bytes),
        })
    }

    fn output_text_bytes(bytes: usize) -> Value {
        serde_json::json!({
            "schema": RUN_OUTPUT_TEXT_SCHEMA,
            "text": "x".repeat(bytes),
        })
    }

    fn max_inline_output_value(limit: u64) -> Value {
        let mut low = 0usize;
        let mut high = limit as usize;
        let mut best = output_text_bytes(0);
        while low <= high {
            let mid = low + ((high - low) / 2);
            let candidate = output_text_bytes(mid);
            let len = serde_json::to_vec(&candidate).unwrap().len() as u64;
            if len <= limit {
                best = candidate;
                low = mid.saturating_add(1);
            } else if mid == 0 {
                break;
            } else {
                high = mid - 1;
            }
        }
        best
    }

    fn init_state(
        root: &str,
        offers: Vec<ConfiguredOffer>,
        adapters: FakeAdapters,
    ) -> ModelProviderState<FakeAdapters> {
        ModelProviderState::from_init(
            BridgeProviderConfig {
                base_path: root.to_string(),
                extra: serde_json::json!({
                    "provider_id": "model-provider",
                    "offers": offers,
                }),
            },
            adapters,
        )
        .unwrap()
    }

    fn run_with_id_for_offer(
        run_id: String,
        binding: RuntimeCreateBinding,
        offer: &ConfiguredOffer,
        created_at_ms: u64,
    ) -> StoredRun {
        StoredRun::new_prepared(
            run_id,
            binding,
            offer.summary(),
            offer.execution_binding_hash().unwrap(),
            created_at_ms,
        )
    }

    fn prepared_run_for_offer(
        binding: RuntimeCreateBinding,
        offer: &ConfiguredOffer,
        created_at_ms: u64,
    ) -> StoredRun {
        run_with_id_for_offer(
            deterministic_run_id(&binding),
            binding,
            offer,
            created_at_ms,
        )
    }

    fn running_run_for_offer(binding: RuntimeCreateBinding, offer: &ConfiguredOffer) -> StoredRun {
        let mut run = prepared_run_for_offer(binding, offer, now_ms());
        run.backend_state = Some(running_backend_state());
        run.status = RunStatus::Running;
        run
    }

    #[test]
    fn duplicate_offer_ids_are_rejected() {
        let adapters = FakeAdapters::default();
        let error = match ModelProviderState::from_init(
            BridgeProviderConfig {
                base_path: temp_root("duplicate"),
                extra: serde_json::json!({
                    "provider_id": "model-provider",
                    "offers": [offer("same"), offer("same")]
                }),
            },
            adapters,
        ) {
            Ok(_) => panic!("duplicate offer ids must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "internal_error");
    }

    #[test]
    fn idempotent_create_returns_stored_run_without_redispatch() {
        let root = temp_root("idempotent");
        let adapters = FakeAdapters {
            dispatch_results: Arc::new(Mutex::new(vec![Ok(DispatchResult::Terminal {
                events: vec![EventSeed {
                    kind: "progress",
                    data: serde_json::json!({
                        "step": "dispatch"
                    }),
                }],
                status: RunStatus::Completed,
                output: Some(serde_json::json!({
                    "schema": RUN_OUTPUT_TEXT_SCHEMA,
                    "text": "done"
                })),
                error: None,
            })])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters.clone());
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let request = RunsCreateRequest {
            op: "runs_create".to_string(),
            offer_id: "local-text".to_string(),
            operation: "text.generate".to_string(),
            input: input.clone(),
            runtime_binding: create_binding("request:one", "local-text", &input),
        };
        let first = state.handle_runs_create(request.clone()).unwrap();
        let second = state.handle_runs_create(request).unwrap();
        assert_eq!(first["status"], "ok");
        assert_eq!(first["data"]["run_id"], second["data"]["run_id"]);
        assert_eq!(*adapters.dispatch_calls.lock().unwrap(), 1);
    }

    #[test]
    fn prepared_run_becomes_settlement_unknown_after_restart() {
        let root = temp_root("prepared");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:prepared", "local-text", &input);
        let offer = state.offers.get("local-text").unwrap().clone();
        let run_id = deterministic_run_id(&binding);
        let mut run = prepared_run_for_offer(binding.clone(), &offer, now_ms());
        append_event(
            &offer.summary(),
            &mut run,
            "prepared",
            serde_json::json!({"offer_id": offer.id, "operation": offer.operation}),
            false,
        )
        .unwrap();
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id,
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        assert_eq!(response["data"]["status"], "settlement_unknown");
    }

    #[test]
    fn exact_current_config_resumes_and_calls_reconcile() {
        let root = temp_root("resume-exact");
        let adapters = FakeAdapters {
            reconcile_results: Arc::new(Mutex::new(vec![Ok(ReconcileResult::StillRunning {
                events: vec![EventSeed {
                    kind: "progress",
                    data: serde_json::json!({"step": "resume"}),
                }],
                backend_state: running_backend_state(),
                status: RunStatus::Running,
            })])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters.clone());
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "resume"
        });
        let binding = create_binding("request:resume-exact", "local-text", &input);
        let run = running_run_for_offer(binding.clone(), &offer("local-text"));
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();

        assert_eq!(response["data"]["status"], "running");
        assert_eq!(*adapters.reconcile_calls.lock().unwrap(), 1);
    }

    #[test]
    fn changed_execution_binding_settles_unknown_without_reconcile() {
        let root = temp_root("binding-changed");
        let adapters = FakeAdapters::default();
        let original_offer = offer("local-text");
        let mut changed_offer = original_offer.clone();
        changed_offer.policy.runtime_ms_limit = original_offer.policy.runtime_ms_limit + 1;
        let mut state = init_state(&root, vec![changed_offer], adapters.clone());
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "binding-change"
        });
        let binding = create_binding("request:binding-change", "local-text", &input);
        let run = running_run_for_offer(binding.clone(), &original_offer);
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        let response_text = serde_json::to_string(&response).unwrap();

        assert_eq!(response["data"]["status"], "settlement_unknown");
        assert_eq!(*adapters.reconcile_calls.lock().unwrap(), 0);
        assert!(!response_text.contains("example.invalid"));
        assert!(!response_text.contains("gpt-test"));

        let stored = state.journal.load_run(&run.run_id).unwrap();
        assert_eq!(stored.status, RunStatus::SettlementUnknown);
        assert!(stored.backend_state.is_none());
    }

    #[test]
    fn removed_offer_settles_unknown_without_reconcile() {
        let root = temp_root("binding-removed");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, Vec::new(), adapters.clone());
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "removed"
        });
        let binding = create_binding("request:binding-removed", "local-text", &input);
        let run = running_run_for_offer(binding.clone(), &offer("local-text"));
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_cancel(RunsCancelRequest {
                op: "runs_cancel".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();

        assert_eq!(response["data"]["status"], "settlement_unknown");
        assert_eq!(*adapters.reconcile_calls.lock().unwrap(), 0);
        assert!(adapters.cancel_allow_send.lock().unwrap().is_empty());
    }

    #[test]
    fn credential_rotation_preserves_restart_binding() {
        let root = temp_root("binding-credentials");
        let adapters = FakeAdapters {
            reconcile_results: Arc::new(Mutex::new(vec![Ok(ReconcileResult::StillRunning {
                events: Vec::new(),
                backend_state: running_backend_state(),
                status: RunStatus::Running,
            })])),
            ..Default::default()
        };
        let original_offer = offer("local-text");
        let mut rotated_offer = original_offer.clone();
        if let AdapterConfig::OpenAiCompatibleText { api_key, .. } = &mut rotated_offer.adapter {
            *api_key = Some("rotated-secret".to_string());
        }
        let mut state = init_state(&root, vec![rotated_offer], adapters.clone());
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "credentials"
        });
        let binding = create_binding("request:binding-credentials", "local-text", &input);
        let run = running_run_for_offer(binding.clone(), &original_offer);
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();

        assert_eq!(response["data"]["status"], "running");
        assert_eq!(*adapters.reconcile_calls.lock().unwrap(), 1);
    }

    #[test]
    fn retained_terminal_run_is_readable_after_offer_change() {
        let root = temp_root("retained-terminal");
        let adapters = FakeAdapters::default();
        let mut changed_offer = offer("local-text");
        if let AdapterConfig::OpenAiCompatibleText { model, .. } = &mut changed_offer.adapter {
            *model = "gpt-next".to_string();
        }
        let mut state = init_state(&root, vec![changed_offer], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "retained"
        });
        let binding = create_binding("request:retained-terminal", "local-text", &input);
        let mut run = prepared_run_for_offer(binding.clone(), &offer("local-text"), now_ms());
        transition_terminal(
            &offer("local-text"),
            &mut run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "done"
            })),
            None,
        )
        .unwrap();
        state.journal.store_run(&run).unwrap();

        let get_response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        let events_response = state
            .handle_runs_events(RunsEventsRequest {
                op: "runs_events".to_string(),
                run_id: run.run_id.clone(),
                after_sequence: Some(0),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();

        assert_eq!(get_response["data"]["status"], "completed");
        assert_eq!(
            events_response["data"]["events"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn retained_terminal_run_is_readable_when_offer_is_absent() {
        let root = temp_root("retained-terminal-absent");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, Vec::new(), adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "retained-absent"
        });
        let binding = create_binding("request:retained-terminal-absent", "local-text", &input);
        let original_offer = offer("local-text");
        let mut run = prepared_run_for_offer(binding.clone(), &original_offer, now_ms());
        transition_terminal(
            &original_offer,
            &mut run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "done"
            })),
            None,
        )
        .unwrap();
        state.journal.store_run(&run).unwrap();

        let get_response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        let events_response = state
            .handle_runs_events(RunsEventsRequest {
                op: "runs_events".to_string(),
                run_id: run.run_id.clone(),
                after_sequence: Some(0),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();

        assert_eq!(get_response["data"]["status"], "completed");
        assert_eq!(
            events_response["data"]["events"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn mismatch_settlement_public_responses_do_not_leak_private_execution_details() {
        let root = temp_root("binding-mismatch-redaction");
        let adapters = FakeAdapters::default();
        let original_offer = offer("local-text");
        let mut changed_offer = original_offer.clone();
        if let AdapterConfig::OpenAiCompatibleText {
            api_url,
            api_key,
            model,
        } = &mut changed_offer.adapter
        {
            *api_url = "https://sentinel.example.test:8443/private-route".to_string();
            *api_key = Some("sentinel-api-key".to_string());
            *model = "sentinel-model".to_string();
        }
        let mut state = init_state(&root, vec![changed_offer], adapters.clone());
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "mismatch-redaction"
        });
        let binding = create_binding("request:mismatch-redaction", "local-text", &input);
        let run = running_run_for_offer(binding.clone(), &original_offer);
        state.journal.store_run(&run).unwrap();

        let get_response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        let events_response = state
            .handle_runs_events(RunsEventsRequest {
                op: "runs_events".to_string(),
                run_id: run.run_id.clone(),
                after_sequence: Some(0),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();

        assert_eq!(get_response["data"]["status"], "settlement_unknown");
        assert_eq!(
            events_response["data"]["events"].as_array().unwrap().len(),
            1
        );
        assert_eq!(*adapters.reconcile_calls.lock().unwrap(), 0);

        let get_text = serde_json::to_string(&get_response).unwrap();
        let events_text = serde_json::to_string(&events_response).unwrap();
        for text in [&get_text, &events_text] {
            assert!(!text.contains("sentinel.example.test"));
            assert!(!text.contains("sentinel-model"));
            assert!(!text.contains("sentinel-api-key"));
            assert!(!text.contains("execution_binding_hash"));
            assert!(!text.contains("backend_state"));
            assert!(!text.contains("endpoint"));
            assert!(!text.contains("route"));
            assert!(!text.contains("8443"));
        }
    }

    #[test]
    fn guessed_run_id_cannot_read_another_owner_run() {
        let root = temp_root("owner");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:owner", "local-text", &input);
        let offer = state.offers.get("local-text").unwrap().clone();
        let run_id = deterministic_run_id(&binding);
        let run = prepared_run_for_offer(binding.clone(), &offer, now_ms());
        state.journal.store_run(&run).unwrap();

        let mut wrong = access_binding(&binding);
        wrong.principal_id = "person:local:other".to_string();
        let error = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id,
                runtime_binding: wrong,
            })
            .unwrap_err();
        assert_eq!(error.code(), "run_not_found");
    }

    #[test]
    fn run_view_redacts_principal_session_grant_and_request_fields() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:redact", "local-text", &input);
        let run = StoredRun::new_prepared(
            "run:redact".to_string(),
            binding,
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        let encoded = serde_json::to_value(run.to_view()).unwrap();
        assert!(encoded.get("principal_id").is_none());
        assert!(encoded.get("session_id").is_none());
        assert!(encoded.get("grant_id").is_none());
        assert!(encoded.get("request_id").is_none());
    }

    #[test]
    fn hashed_run_filename_load_verifies_exact_identifier() {
        let root = temp_root("hashed");
        let journal = RunJournal::open(std::path::PathBuf::from(&root)).unwrap();
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:hash", "local-text", &input);
        let other_binding = create_binding(
            "request:hash-other",
            "local-text",
            &serde_json::json!({
                "schema": "elastos.model.input.text/v1",
                "prompt": "hello"
            }),
        );
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        append_event(
            &offer("local-text"),
            &mut run,
            "prepared",
            serde_json::json!({}),
            false,
        )
        .unwrap();
        let wrong_path = crate::journal::hashed_path(
            std::path::Path::new(&root).join("runs").as_path(),
            &deterministic_run_id(&other_binding),
        );
        std::fs::create_dir_all(std::path::Path::new(&root).join("runs")).unwrap();
        std::fs::write(wrong_path, serde_json::to_vec_pretty(&run).unwrap()).unwrap();
        let error = journal
            .load_run(&deterministic_run_id(&other_binding))
            .unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
    }

    #[test]
    fn events_cursor_is_monotonic() {
        let root = temp_root("cursor");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:cursor", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        let current_offer = offer("local-text");
        append_event(
            &current_offer,
            &mut run,
            "prepared",
            serde_json::json!({}),
            false,
        )
        .unwrap();
        append_event(
            &current_offer,
            &mut run,
            "progress",
            serde_json::json!({}),
            false,
        )
        .unwrap();
        transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "ok"
            })),
            None,
        )
        .unwrap();
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_events(RunsEventsRequest {
                op: "runs_events".to_string(),
                run_id: run.run_id,
                after_sequence: Some(1),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        assert_eq!(response["data"]["events"].as_array().unwrap().len(), 2);
        assert_eq!(response["data"]["next_cursor"], serde_json::json!(3));
        assert_eq!(response["data"]["has_more"], serde_json::json!(false));
    }

    #[test]
    fn event_count_limit_fails_before_mutation() {
        let mut current_offer = offer("local-text");
        current_offer.policy.event_bytes_limit = MAX_EVENT_BYTES_LIMIT;
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:event-count", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );

        for index in 0..(MAX_RUN_EVENT_COUNT_LIMIT - 1) {
            append_event(
                &current_offer,
                &mut run,
                "progress",
                serde_json::json!({ "index": index }),
                false,
            )
            .unwrap();
        }
        let previous_sequence = run.next_sequence;
        let error = append_event(
            &current_offer,
            &mut run,
            "progress",
            serde_json::json!({ "index": MAX_RUN_EVENT_COUNT_LIMIT }),
            false,
        )
        .unwrap_err();
        assert_eq!(error.code(), "policy_limit");
        assert_eq!(run.events.len(), MAX_RUN_EVENT_COUNT_LIMIT - 1);
        assert_eq!(run.next_sequence, previous_sequence);
    }

    #[test]
    fn aggregate_event_limit_fails_before_mutation() {
        let mut current_offer = offer("local-text");
        current_offer.policy.event_bytes_limit = MAX_EVENT_BYTES_LIMIT;
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:event-aggregate", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let payload =
            event_data_bytes((MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT as usize / 3) + 8 * 1024);

        append_event(&current_offer, &mut run, "progress", payload.clone(), false).unwrap();
        let previous_sequence = run.next_sequence;
        let error = append_event(&current_offer, &mut run, "progress", payload, false).unwrap_err();

        assert_eq!(error.code(), "policy_limit");
        assert_eq!(run.events.len(), 1);
        assert_eq!(run.next_sequence, previous_sequence);
        assert!(run_event_bytes_total(&run.events).unwrap() <= MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT);
    }

    #[test]
    fn runs_events_pages_without_gaps_or_duplicates() {
        let root = temp_root("event-pages");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:event-pages", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        let current_offer = offer("local-text");
        for index in 0..=MAX_RUN_EVENTS_PAGE_COUNT_LIMIT {
            append_event(
                &current_offer,
                &mut run,
                "progress",
                serde_json::json!({ "index": index }),
                false,
            )
            .unwrap();
        }
        transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "done"
            })),
            None,
        )
        .unwrap();
        state.journal.store_run(&run).unwrap();

        let first = state
            .handle_runs_events(RunsEventsRequest {
                op: "runs_events".to_string(),
                run_id: run.run_id.clone(),
                after_sequence: Some(0),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        let first_events = first["data"]["events"].as_array().unwrap();
        assert_eq!(first_events.len(), MAX_RUN_EVENTS_PAGE_COUNT_LIMIT);
        assert_eq!(first["data"]["has_more"], serde_json::json!(true));
        assert_eq!(
            first["data"]["next_cursor"],
            serde_json::json!(MAX_RUN_EVENTS_PAGE_COUNT_LIMIT as u64)
        );
        assert_eq!(
            first_events.first().unwrap()["sequence"],
            serde_json::json!(1)
        );
        assert_eq!(
            first_events.last().unwrap()["sequence"],
            serde_json::json!(MAX_RUN_EVENTS_PAGE_COUNT_LIMIT as u64)
        );

        let second = state
            .handle_runs_events(RunsEventsRequest {
                op: "runs_events".to_string(),
                run_id: run.run_id.clone(),
                after_sequence: Some(MAX_RUN_EVENTS_PAGE_COUNT_LIMIT as u64),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        let second_events = second["data"]["events"].as_array().unwrap();
        assert_eq!(second_events.len(), 2);
        assert_eq!(second["data"]["has_more"], serde_json::json!(false));
        assert_eq!(
            second["data"]["next_cursor"],
            serde_json::json!((MAX_RUN_EVENTS_PAGE_COUNT_LIMIT + 2) as u64)
        );
        assert_eq!(
            second_events[0]["sequence"],
            serde_json::json!((MAX_RUN_EVENTS_PAGE_COUNT_LIMIT + 1) as u64)
        );
        assert_eq!(
            second_events[1]["sequence"],
            serde_json::json!((MAX_RUN_EVENTS_PAGE_COUNT_LIMIT + 2) as u64)
        );
    }

    #[test]
    fn one_large_valid_event_fits_single_page() {
        let root = temp_root("large-event-page");
        let adapters = FakeAdapters::default();
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:large-event-page", "local-text", &input);
        let mut current_offer = offer("local-text");
        current_offer.policy.event_bytes_limit = MAX_EVENT_BYTES_LIMIT;
        current_offer.policy.inline_output_bytes_limit = MAX_INLINE_OUTPUT_BYTES_LIMIT;
        let mut state = init_state(&root, vec![current_offer.clone()], adapters);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let output = max_inline_output_value(MAX_INLINE_OUTPUT_BYTES_LIMIT);
        transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(output.clone()),
            None,
        )
        .unwrap();
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_events(RunsEventsRequest {
                op: "runs_events".to_string(),
                run_id: run.run_id,
                after_sequence: Some(0),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        let events = response["data"]["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(response["data"]["has_more"], serde_json::json!(false));
        assert_eq!(response["data"]["next_cursor"], serde_json::json!(1));
    }

    #[test]
    fn runs_events_empty_tail_is_not_an_error() {
        let root = temp_root("event-tail");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:event-tail", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        let current_offer = offer("local-text");
        append_event(
            &current_offer,
            &mut run,
            "prepared",
            serde_json::json!({}),
            false,
        )
        .unwrap();
        append_event(
            &current_offer,
            &mut run,
            "progress",
            serde_json::json!({}),
            false,
        )
        .unwrap();
        transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "done"
            })),
            None,
        )
        .unwrap();
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_events(RunsEventsRequest {
                op: "runs_events".to_string(),
                run_id: run.run_id,
                after_sequence: Some(10),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        assert_eq!(response["data"]["events"], serde_json::json!([]));
        assert_eq!(response["data"]["has_more"], serde_json::json!(false));
        assert_eq!(response["data"]["next_cursor"], serde_json::json!(3));
    }

    #[test]
    fn terminal_event_can_settle_after_non_terminal_count_reserve_is_reached() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:terminal-count-reserve", "local-text", &input);
        let mut current_offer = offer("local-text");
        current_offer.policy.event_bytes_limit = MAX_EVENT_BYTES_LIMIT;
        current_offer.policy.inline_output_bytes_limit = MAX_INLINE_OUTPUT_BYTES_LIMIT;
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );

        for index in 0..(MAX_RUN_EVENT_COUNT_LIMIT - 1) {
            append_event(
                &current_offer,
                &mut run,
                "progress",
                serde_json::json!({ "index": index }),
                false,
            )
            .unwrap();
        }

        transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(output_text_bytes(32)),
            None,
        )
        .unwrap();

        assert_eq!(run.events.len(), MAX_RUN_EVENT_COUNT_LIMIT);
        assert!(run.events.last().unwrap().terminal);
        assert_eq!(run.status, RunStatus::Completed);
    }

    #[test]
    fn rejected_progress_update_leaves_durable_state_byte_identical_and_terminal_can_still_settle()
    {
        let root = temp_root("worker-budget-reject");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let current_offer = offer("local-text");
        let binding = create_binding("request:worker-budget-reject", "local-text", &input);
        let run_id = deterministic_run_id(&binding);
        let mut run = StoredRun::new_prepared(
            run_id.clone(),
            binding.clone(),
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        run.status = RunStatus::Running;
        run.backend_state = Some(serialize_local_text_backend_state(false).unwrap());

        for index in 0..(MAX_RUN_EVENT_COUNT_LIMIT - 1) {
            append_event(
                &current_offer,
                &mut run,
                "progress",
                serde_json::json!({ "index": index }),
                false,
            )
            .unwrap();
        }
        state.journal.store_run(&run).unwrap();

        let path = run_path(&root, &run_id);
        let before = std::fs::read(&path).unwrap();
        let error = state
            .apply_worker_reconcile_result(
                &run_id,
                WorkerApplyGuard::None,
                ReconcileResult::StillRunning {
                    events: vec![EventSeed {
                        kind: "text_delta",
                        data: serde_json::json!({ "text": "x" }),
                    }],
                    backend_state: serialize_local_text_backend_state(false).unwrap(),
                    status: RunStatus::Running,
                },
            )
            .unwrap_err();
        assert_eq!(error.code(), "policy_limit");
        let after_rejection = std::fs::read(&path).unwrap();
        assert_eq!(after_rejection, before);

        state
            .apply_worker_reconcile_result(
                &run_id,
                WorkerApplyGuard::None,
                ReconcileResult::Terminal {
                    events: Vec::new(),
                    status: RunStatus::Failed,
                    output: None,
                    error: Some(RunError {
                        class: ErrorClass::ContextRejected,
                        code: "context_rejected".to_string(),
                        message: "model run could not continue".to_string(),
                    }),
                },
            )
            .unwrap();

        let settled = state.journal.load_run(&run_id).unwrap();
        assert_eq!(settled.status, RunStatus::Failed);
        assert_eq!(settled.events.len(), MAX_RUN_EVENT_COUNT_LIMIT);
        assert_eq!(settled.events.last().unwrap().kind, "failed");
        assert!(settled.events.last().unwrap().terminal);
        assert!(settled.output.is_none());
        assert!(settled.backend_state.is_none());
        assert_eq!(
            settled.error,
            Some(RunError {
                class: ErrorClass::ContextRejected,
                code: "context_rejected".to_string(),
                message: "model run could not continue".to_string(),
            })
        );
    }

    #[test]
    fn stale_status_terminal_cannot_settle_before_newer_cancel_reservation() {
        let root = temp_root("status-terminal-stale-cancel");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let current_offer = offer("local-text");
        let binding = create_binding("request:status-terminal-stale-cancel", "local-text", &input);
        let run_id = deterministic_run_id(&binding);
        let mut run = StoredRun::new_prepared(
            run_id.clone(),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        run.status = RunStatus::Reconciling;
        let stale_backend_state = running_backend_state();
        run.backend_state = Some(reserved_cancel_backend_state(
            now_ms().saturating_add(1_000),
            false,
        ));
        state.journal.store_run(&run).unwrap();

        state
            .apply_worker_reconcile_result(
                &run_id,
                WorkerApplyGuard::HttpArtifactBackendState {
                    backend_state: stale_backend_state,
                },
                ReconcileResult::Terminal {
                    events: Vec::new(),
                    status: RunStatus::Completed,
                    output: Some(serde_json::json!({
                        "schema": RUN_OUTPUT_TEXT_SCHEMA,
                        "text": "wrong",
                    })),
                    error: None,
                },
            )
            .unwrap();

        let stored = state.journal.load_run(&run_id).unwrap();
        assert_eq!(stored.status, RunStatus::Reconciling);
        assert_eq!(
            stored.backend_state,
            Some(reserved_cancel_backend_state(
                stored
                    .backend_state
                    .as_ref()
                    .and_then(|value| value.get("cancel_deadline_ms"))
                    .and_then(Value::as_u64)
                    .unwrap(),
                false,
            ))
        );
        assert_eq!(stored.events.len(), 0);
        assert!(stored.output.is_none());
    }

    #[test]
    fn stale_status_terminal_for_old_job_id_cannot_settle_current_run() {
        let root = temp_root("status-terminal-old-job");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let current_offer = offer("local-text");
        let binding = create_binding("request:status-terminal-old-job", "local-text", &input);
        let run_id = deterministic_run_id(&binding);
        let mut run = StoredRun::new_prepared(
            run_id.clone(),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        run.status = RunStatus::Running;
        run.backend_state = Some(serde_json::json!({
            "schema": "elastos.model.provider-http-job-state/v1",
            "job_id": "job-2",
            "next_poll_at_ms": 0,
            "cancel_requested": false,
            "cancel_sent": false,
            "cancel_deadline_ms": null,
        }));
        state.journal.store_run(&run).unwrap();

        state
            .apply_worker_reconcile_result(
                &run_id,
                WorkerApplyGuard::HttpArtifactBackendState {
                    backend_state: serde_json::json!({
                        "schema": "elastos.model.provider-http-job-state/v1",
                        "job_id": "job-1",
                        "next_poll_at_ms": 0,
                        "cancel_requested": false,
                        "cancel_sent": false,
                        "cancel_deadline_ms": null,
                    }),
                },
                ReconcileResult::Terminal {
                    events: Vec::new(),
                    status: RunStatus::Failed,
                    output: None,
                    error: Some(RunError {
                        class: ErrorClass::BackendFailed,
                        code: "backend_failed".to_string(),
                        message: "model backend failed".to_string(),
                    }),
                },
            )
            .unwrap();

        let stored = state.journal.load_run(&run_id).unwrap();
        assert_eq!(stored.status, RunStatus::Running);
        assert_eq!(
            stored.backend_state.as_ref().unwrap()["job_id"],
            serde_json::json!("job-2")
        );
        assert_eq!(stored.events.len(), 0);
        assert!(stored.error.is_none());
    }

    #[test]
    fn current_cancel_reserved_status_worker_can_settle_cancelled() {
        let root = temp_root("status-terminal-current-cancel");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let current_offer = offer("local-text");
        let binding = create_binding(
            "request:status-terminal-current-cancel",
            "local-text",
            &input,
        );
        let run_id = deterministic_run_id(&binding);
        let mut run = StoredRun::new_prepared(
            run_id.clone(),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        run.status = RunStatus::Reconciling;
        let reserved_backend_state =
            reserved_cancel_backend_state(now_ms().saturating_add(1_000), true);
        run.backend_state = Some(reserved_backend_state.clone());
        state.journal.store_run(&run).unwrap();

        state
            .apply_worker_reconcile_result(
                &run_id,
                WorkerApplyGuard::HttpArtifactBackendState {
                    backend_state: reserved_backend_state,
                },
                ReconcileResult::Terminal {
                    events: Vec::new(),
                    status: RunStatus::Cancelled,
                    output: None,
                    error: Some(RunError {
                        class: ErrorClass::Cancelled,
                        code: "cancelled".to_string(),
                        message: "model run was cancelled".to_string(),
                    }),
                },
            )
            .unwrap();

        let stored = state.journal.load_run(&run_id).unwrap();
        assert_eq!(stored.status, RunStatus::Cancelled);
        assert_eq!(stored.events.last().unwrap().kind, "cancelled");
        assert!(stored.backend_state.is_none());
    }

    #[test]
    fn terminal_event_can_use_reserved_aggregate_budget() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:terminal-aggregate-reserve", "local-text", &input);
        let mut current_offer = offer("local-text");
        current_offer.policy.event_bytes_limit = MAX_EVENT_BYTES_LIMIT;
        current_offer.policy.inline_output_bytes_limit = MAX_INLINE_OUTPUT_BYTES_LIMIT;
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );

        let payload = event_data_bytes(120 * 1024);
        append_event(&current_offer, &mut run, "progress", payload.clone(), false).unwrap();
        append_event(&current_offer, &mut run, "progress", payload.clone(), false).unwrap();
        let error = append_event(&current_offer, &mut run, "progress", payload, false).unwrap_err();
        assert_eq!(error.code(), "policy_limit");
        assert!(run.events.len() < MAX_RUN_EVENT_COUNT_LIMIT - 1);

        transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(output_text_bytes(32)),
            None,
        )
        .unwrap();

        assert!(run.events.last().unwrap().terminal);
        assert!(run_event_bytes_total(&run.events).unwrap() <= MAX_RUN_EVENT_AGGREGATE_BYTES_LIMIT);
    }

    #[test]
    fn oversized_inline_output_fails_before_terminal_event_mutation() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:oversized-inline-output", "local-text", &input);
        let mut current_offer = offer("local-text");
        current_offer.policy.event_bytes_limit = MAX_EVENT_BYTES_LIMIT;
        current_offer.policy.inline_output_bytes_limit = MAX_INLINE_OUTPUT_BYTES_LIMIT;
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let oversized = output_text_bytes(MAX_INLINE_OUTPUT_BYTES_LIMIT as usize);
        assert!(
            serde_json::to_vec(&oversized).unwrap().len() as u64 > MAX_INLINE_OUTPUT_BYTES_LIMIT
        );

        let error = transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(oversized),
            None,
        )
        .unwrap_err();

        assert_eq!(error.code(), "policy_limit");
        assert_eq!(run.events.len(), 0);
        assert_eq!(run.next_sequence, 1);
        assert_eq!(run.status, RunStatus::Prepared);
        assert!(run.output.is_none());
    }

    #[test]
    fn completed_without_output_is_rejected_before_mutation() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:missing-completed-output", "local-text", &input);
        let current_offer = offer("local-text");
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let before = serde_json::to_vec(&run).unwrap();

        let error = transition_terminal(&current_offer, &mut run, RunStatus::Completed, None, None)
            .unwrap_err();

        assert_eq!(error.code(), "internal_error");
        assert_eq!(serde_json::to_vec(&run).unwrap(), before);
    }

    #[test]
    fn invalid_terminal_status_leaves_run_unchanged() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:invalid-terminal-status", "local-text", &input);
        let current_offer = offer("local-text");
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let before = serde_json::to_vec(&run).unwrap();

        let error = transition_terminal(&current_offer, &mut run, RunStatus::Prepared, None, None)
            .unwrap_err();

        assert_eq!(error.code(), "internal_error");
        assert_eq!(serde_json::to_vec(&run).unwrap(), before);
    }

    #[test]
    fn missing_terminal_error_leaves_run_unchanged() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:missing-terminal-error", "local-text", &input);
        let current_offer = offer("local-text");
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let before = serde_json::to_vec(&run).unwrap();

        let error = transition_terminal(&current_offer, &mut run, RunStatus::Failed, None, None)
            .unwrap_err();

        assert_eq!(error.code(), "internal_error");
        assert_eq!(serde_json::to_vec(&run).unwrap(), before);
    }

    #[test]
    fn maximum_valid_inline_output_fits_terminal_event_and_run_view_frame() {
        const RESPONSE_FRAME_BYTES: usize = 256 * 1024;

        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:max-inline-output", "local-text", &input);
        let mut current_offer = offer("local-text");
        current_offer.policy.event_bytes_limit = MAX_EVENT_BYTES_LIMIT;
        current_offer.policy.inline_output_bytes_limit = MAX_INLINE_OUTPUT_BYTES_LIMIT;
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let output = max_inline_output_value(MAX_INLINE_OUTPUT_BYTES_LIMIT);
        assert_eq!(
            serde_json::to_vec(&output).unwrap().len() as u64,
            MAX_INLINE_OUTPUT_BYTES_LIMIT
        );

        transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(output.clone()),
            None,
        )
        .unwrap();

        let terminal_event_bytes = encoded_run_event_len(run.events.last().unwrap()).unwrap();
        assert!(terminal_event_bytes <= MAX_EVENT_BYTES_LIMIT);

        let response = ok_response(serde_json::to_value(run.to_view()).unwrap());
        let response_bytes = serde_json::to_vec(&response).unwrap();
        assert!(response_bytes.len() < RESPONSE_FRAME_BYTES);
    }

    #[test]
    fn concurrency_limit_is_enforced() {
        let root = temp_root("concurrency");
        let adapters = FakeAdapters {
            dispatch_results: Arc::new(Mutex::new(vec![
                Ok(DispatchResult::Running {
                    events: vec![EventSeed {
                        kind: "dispatched",
                        data: serde_json::json!({}),
                    }],
                    backend_state: serde_json::json!({"job_id":"job-1"}),
                }),
                Ok(DispatchResult::Running {
                    events: vec![EventSeed {
                        kind: "dispatched",
                        data: serde_json::json!({}),
                    }],
                    backend_state: serde_json::json!({"job_id":"job-2"}),
                }),
            ])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input1 = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"one"});
        let input2 = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"two"});
        let first = state
            .handle_runs_create(RunsCreateRequest {
                op: "runs_create".to_string(),
                offer_id: "local-text".to_string(),
                operation: "text.generate".to_string(),
                input: input1.clone(),
                runtime_binding: create_binding("request:one", "local-text", &input1),
            })
            .unwrap();
        let second = state
            .handle_runs_create(RunsCreateRequest {
                op: "runs_create".to_string(),
                offer_id: "local-text".to_string(),
                operation: "text.generate".to_string(),
                input: input2.clone(),
                runtime_binding: create_binding("request:two", "local-text", &input2),
            })
            .unwrap();
        assert_eq!(first["data"]["status"], "running");
        assert_eq!(second["data"]["status"], "failed");
        assert_eq!(
            second["data"]["terminal"]["error"]["class"],
            "selection_unavailable"
        );
    }

    #[test]
    fn expired_terminal_run_fails_closed() {
        let root = temp_root("retention");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"hello"});
        let binding = create_binding("request:retention", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        transition_terminal(
            &offer("local-text"),
            &mut run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "ok"
            })),
            None,
        )
        .unwrap();
        expire_terminal_run(&mut run);
        state.journal.store_run(&run).unwrap();
        let error = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id,
                runtime_binding: access_binding(&binding),
            })
            .unwrap_err();
        assert_eq!(error.code(), "run_not_found");
    }

    #[test]
    fn runs_create_prunes_expired_unrelated_terminal() {
        let root = temp_root("prune-create");
        let adapters = FakeAdapters {
            dispatch_results: Arc::new(Mutex::new(vec![Ok(DispatchResult::Running {
                events: vec![EventSeed {
                    kind: "dispatched",
                    data: serde_json::json!({}),
                }],
                backend_state: serde_json::json!({"job_id":"job-1"}),
            })])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters);

        let old_input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"old"});
        let old_binding = create_binding("request:old-expired", "local-text", &old_input);
        let mut old_run = StoredRun::new_prepared(
            deterministic_run_id(&old_binding),
            old_binding,
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        transition_terminal(
            &offer("local-text"),
            &mut old_run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "old"
            })),
            None,
        )
        .unwrap();
        expire_terminal_run(&mut old_run);
        let old_path = run_path(&root, &old_run.run_id);
        state.journal.store_run(&old_run).unwrap();

        let input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"new"});
        let response = state
            .handle_runs_create(RunsCreateRequest {
                op: "runs_create".to_string(),
                offer_id: "local-text".to_string(),
                operation: "text.generate".to_string(),
                input: input.clone(),
                runtime_binding: create_binding("request:new", "local-text", &input),
            })
            .unwrap();

        assert_eq!(response["data"]["status"], "running");
        assert!(!old_path.exists());
    }

    #[test]
    fn invalid_runs_create_does_not_prune_expired_unrelated_file() {
        let root = temp_root("invalid-create-no-prune");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);

        let old_input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"old"});
        let old_binding = create_binding("request:old-invalid-create", "local-text", &old_input);
        let mut old_run = StoredRun::new_prepared(
            deterministic_run_id(&old_binding),
            old_binding,
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        transition_terminal(
            &offer("local-text"),
            &mut old_run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "old"
            })),
            None,
        )
        .unwrap();
        expire_terminal_run(&mut old_run);
        let old_path = run_path(&root, &old_run.run_id);
        state.journal.store_run(&old_run).unwrap();

        let invalid_input =
            serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"new"});
        let error = state
            .handle_runs_create(RunsCreateRequest {
                op: "runs_create".to_string(),
                offer_id: "local-text".to_string(),
                operation: "image.generate".to_string(),
                input: invalid_input.clone(),
                runtime_binding: create_binding(
                    "request:new-invalid",
                    "local-text",
                    &invalid_input,
                ),
            })
            .unwrap_err();
        assert_eq!(error.code(), "invalid_request");
        assert!(old_path.exists());
    }

    #[test]
    fn owner_access_prunes_newly_expired_terminal_run() {
        let root = temp_root("owner-prune");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"hello"});
        let binding = create_binding("request:owner-prune", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        transition_terminal(
            &offer("local-text"),
            &mut run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "ok"
            })),
            None,
        )
        .unwrap();
        expire_terminal_run(&mut run);
        let path = run_path(&root, &run.run_id);
        state.journal.store_run(&run).unwrap();

        let error = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap_err();
        assert_eq!(error.code(), "run_not_found");
        assert!(!path.exists());
    }

    #[test]
    fn wrong_owner_cannot_trigger_expired_run_deletion() {
        let root = temp_root("wrong-owner-prune");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"hello"});
        let binding = create_binding("request:wrong-owner-prune", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        transition_terminal(
            &offer("local-text"),
            &mut run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "ok"
            })),
            None,
        )
        .unwrap();
        expire_terminal_run(&mut run);
        let path = run_path(&root, &run.run_id);
        state.journal.store_run(&run).unwrap();

        let mut wrong = access_binding(&binding);
        wrong.principal_id = "person:local:other".to_string();
        let error = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: wrong,
            })
            .unwrap_err();
        assert_eq!(error.code(), "run_not_found");
        assert!(path.exists());
    }

    #[test]
    fn output_stays_typed_and_untrusted() {
        let current_offer = offer("local-text");
        let mut run = StoredRun::new_prepared(
            "run:artifact".to_string(),
            create_binding(
                "request:artifact",
                "local-text",
                &serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"hello"}),
            ),
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_OBJECT_SCHEMA,
                "uri": "elastos://object/example"
            })),
            None,
        )
        .unwrap();
        let view = serde_json::to_value(run.to_view()).unwrap();
        assert!(view["terminal"]["output"]["uri"]
            .as_str()
            .unwrap()
            .starts_with("elastos://object/"));
        assert!(view["terminal"]["output"].get("path").is_none());
    }

    #[test]
    fn conflicting_retry_reuses_run_id_and_fails_by_stable_request_fingerprint() {
        let root = temp_root("conflict");
        let adapters = FakeAdapters::default();
        let mut state = init_state(
            &root,
            vec![offer("local-text"), offer("other-offer")],
            adapters,
        );
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let conflict_input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "changed"
        });
        let request_id = "request:conflict";
        let binding = create_binding(request_id, "local-text", &input);
        let conflict_binding = create_binding(request_id, "local-text", &conflict_input);
        let run_id = deterministic_run_id(&binding);
        assert_eq!(run_id, deterministic_run_id(&conflict_binding));

        let offer = offer("local-text");
        let mut run = StoredRun::new_prepared(
            run_id.clone(),
            binding.clone(),
            offer.summary(),
            offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        append_event(
            &offer,
            &mut run,
            "prepared",
            serde_json::json!({"offer_id": "local-text", "operation": "text.generate"}),
            false,
        )
        .unwrap();
        state.journal.store_run(&run).unwrap();

        for conflicting_request in [
            RunsCreateRequest {
                op: "runs_create".to_string(),
                offer_id: "local-text".to_string(),
                operation: "text.generate".to_string(),
                input: conflict_input,
                runtime_binding: conflict_binding,
            },
            {
                let mut binding = create_binding(request_id, "local-text", &input);
                binding.operation = "image.generate".to_string();
                RunsCreateRequest {
                    op: "runs_create".to_string(),
                    offer_id: "local-text".to_string(),
                    operation: "image.generate".to_string(),
                    input: input.clone(),
                    runtime_binding: binding,
                }
            },
            {
                let mut binding = create_binding(request_id, "other-offer", &input);
                binding.operation = "text.generate".to_string();
                RunsCreateRequest {
                    op: "runs_create".to_string(),
                    offer_id: "other-offer".to_string(),
                    operation: "text.generate".to_string(),
                    input: input.clone(),
                    runtime_binding: binding,
                }
            },
        ] {
            let error = state.handle_runs_create(conflicting_request).unwrap_err();
            assert_eq!(error.code(), "invalid_request");
        }
    }

    #[test]
    fn create_retry_after_session_and_grant_rotation_returns_same_run_without_redispatch() {
        let root = temp_root("rotated-create");
        let adapters = FakeAdapters {
            dispatch_results: Arc::new(Mutex::new(vec![Ok(DispatchResult::Terminal {
                events: vec![EventSeed {
                    kind: "dispatched",
                    data: serde_json::json!({"offer_id": "local-text"}),
                }],
                status: RunStatus::Completed,
                output: Some(serde_json::json!({
                    "schema": RUN_OUTPUT_TEXT_SCHEMA,
                    "text": "done"
                })),
                error: None,
            })])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters.clone());
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let original_binding = create_binding("request:rotated-create", "local-text", &input);
        let rotated_binding =
            rotate_create_audit(&original_binding, "session:rotated", "grant:rotated");

        let first = state
            .handle_runs_create(RunsCreateRequest {
                op: "runs_create".to_string(),
                offer_id: "local-text".to_string(),
                operation: "text.generate".to_string(),
                input: input.clone(),
                runtime_binding: original_binding.clone(),
            })
            .unwrap();
        let second = state
            .handle_runs_create(RunsCreateRequest {
                op: "runs_create".to_string(),
                offer_id: "local-text".to_string(),
                operation: "text.generate".to_string(),
                input,
                runtime_binding: rotated_binding,
            })
            .unwrap();

        assert_eq!(first["data"]["run_id"], second["data"]["run_id"]);
        assert_eq!(*adapters.dispatch_calls.lock().unwrap(), 1);
        let stored = state
            .journal
            .load_run(first["data"]["run_id"].as_str().unwrap())
            .unwrap();
        assert_eq!(stored.runtime_binding.session_id, "session:test");
        assert_eq!(stored.runtime_binding.grant_id, "grant:test");
    }

    #[test]
    fn rotated_session_grant_and_request_can_access_existing_run() {
        let root = temp_root("rotated-access");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:access-rotate", "local-text", &input);
        let run_id = deterministic_run_id(&binding);
        let mut run = StoredRun::new_prepared(
            run_id.clone(),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        transition_terminal(
            &offer("local-text"),
            &mut run,
            RunStatus::Cancelled,
            None,
            Some(RunError {
                class: ErrorClass::Cancelled,
                code: "cancelled".to_string(),
                message: "model run was cancelled".to_string(),
            }),
        )
        .unwrap();
        state.journal.store_run(&run).unwrap();

        let rotated_access = rotate_access_invocation(
            &binding,
            "session:fresh",
            "grant:replacement",
            "request:access-fresh",
        );
        let get = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run_id.clone(),
                runtime_binding: rotated_access.clone(),
            })
            .unwrap();
        let events = state
            .handle_runs_events(RunsEventsRequest {
                op: "runs_events".to_string(),
                run_id: run_id.clone(),
                after_sequence: Some(0),
                runtime_binding: rotated_access.clone(),
            })
            .unwrap();
        let cancel = state
            .handle_runs_cancel(RunsCancelRequest {
                op: "runs_cancel".to_string(),
                run_id: run_id.clone(),
                runtime_binding: rotated_access.clone(),
            })
            .unwrap();

        assert_eq!(get["data"]["status"], "cancelled");
        assert_eq!(events["data"]["run_id"], run_id);
        assert_eq!(cancel["data"]["status"], "cancelled");

        let public = [
            serde_json::to_string(&get).unwrap(),
            serde_json::to_string(&events).unwrap(),
            serde_json::to_string(&cancel).unwrap(),
        ]
        .join("\n");
        for hidden in [
            "session:test",
            "grant:test",
            "request:access-rotate",
            "session:fresh",
            "grant:replacement",
            "request:access-fresh",
        ] {
            assert!(!public.contains(hidden), "public response leaked {hidden}");
        }
        let stored = state.journal.load_run(&run_id).unwrap();
        assert_eq!(stored.runtime_binding.session_id, "session:test");
        assert_eq!(stored.runtime_binding.grant_id, "grant:test");
    }

    #[test]
    fn access_binding_requires_exact_principal_capsule_and_run_id() {
        let root = temp_root("access-scope");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:access", "local-text", &input);
        let run_id = deterministic_run_id(&binding);
        let run = StoredRun::new_prepared(
            run_id.clone(),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        state.journal.store_run(&run).unwrap();

        for wrong_binding in [
            {
                let mut wrong = access_binding(&binding);
                wrong.principal_id = "person:local:other".to_string();
                wrong
            },
            {
                let mut wrong = access_binding(&binding);
                wrong.capsule_id = "other-capsule".to_string();
                wrong
            },
        ] {
            let error = state
                .handle_runs_get(RunsGetRequest {
                    op: "runs_get".to_string(),
                    run_id: run_id.clone(),
                    runtime_binding: wrong_binding,
                })
                .unwrap_err();
            assert_eq!(error.code(), "run_not_found");
        }

        let mut wrong = access_binding(&binding);
        wrong.run_id = deterministic_run_id(&create_binding(
            "request:other-run",
            &binding.offer_id,
            &serde_json::json!({
                "schema": "elastos.model.input.text/v1",
                "prompt": "other"
            }),
        ));
        let error = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id,
                runtime_binding: wrong,
            })
            .unwrap_err();
        assert_eq!(error.code(), "invalid_request");
    }

    #[test]
    fn rotated_session_grant_and_request_can_access_same_exact_run() {
        let root = temp_root("rotated-access");
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:stable-access", "local-text", &input);
        let run_id = deterministic_run_id(&binding);
        let mut state = init_state(&root, vec![offer("local-text")], FakeAdapters::default());
        let run = StoredRun::new_prepared(
            run_id.clone(),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run_id.clone(),
                runtime_binding: rotate_access_invocation(
                    &binding,
                    "session:rotated",
                    "grant:rotated",
                    "request:fresh-access",
                ),
            })
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["run_id"], run_id);
        let stored = state.journal.load_run(&run.run_id).unwrap();
        assert_eq!(stored.runtime_binding.session_id, binding.session_id);
        assert_eq!(stored.runtime_binding.grant_id, binding.grant_id);
    }

    #[test]
    fn access_binding_rejects_legacy_offer_and_operation_fields() {
        let run_id = "run:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let error = serde_json::from_value::<RunsGetRequest>(serde_json::json!({
            "op": "runs_get",
            "run_id": run_id,
            "runtime_binding": {
                "schema": RUNTIME_ACCESS_BINDING_SCHEMA,
                "principal_id": "person:local:test",
                "session_id": "session:test",
                "capsule_id": "assistant",
                "grant_id": "grant:test",
                "request_id": "request:test",
                "run_id": run_id,
                "offer_id": "local-text",
                "operation": "text.generate"
            }
        }))
        .unwrap_err()
        .to_string();

        assert!(
            error.contains("unknown field `offer_id`")
                || error.contains("unknown field `operation`")
        );
    }

    #[test]
    fn cancel_reservation_is_stored_before_io() {
        let root = temp_root("cancel-reserve");
        let adapters = FakeAdapters {
            reconcile_results: Arc::new(Mutex::new(vec![Ok(ReconcileResult::StillRunning {
                events: Vec::new(),
                backend_state: running_backend_state(),
                status: RunStatus::Running,
            })])),
            reserve_cancel_results: Arc::new(Mutex::new(vec![Ok(CancelReservation {
                backend_state: reserved_cancel_backend_state(55, true),
                allow_send: true,
            })])),
            cancel_results: Arc::new(Mutex::new(vec![Ok(CancelResult::Reconciling {
                events: Vec::new(),
                backend_state: reserved_cancel_backend_state(55, true),
            })])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters.clone());
        let input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"hello"});
        let binding = create_binding("request:cancel-reserve", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        run.backend_state = Some(running_backend_state());
        run.status = RunStatus::Running;
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_cancel(RunsCancelRequest {
                op: "runs_cancel".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();

        let stored = state.journal.load_run(&run.run_id).unwrap();
        assert_eq!(response["data"]["status"], "reconciling");
        assert_eq!(stored.status, RunStatus::Reconciling);
        assert_eq!(
            stored.backend_state.as_ref().unwrap()["cancel_requested"],
            serde_json::json!(true)
        );
        assert_eq!(
            stored.backend_state.as_ref().unwrap()["cancel_sent"],
            serde_json::json!(true)
        );
        assert_eq!(*adapters.cancel_allow_send.lock().unwrap(), vec![true]);
    }

    #[test]
    fn reserved_cancel_retry_does_not_send_again_or_extend_deadline() {
        let root = temp_root("cancel-retry");
        let first_adapters = FakeAdapters {
            reconcile_results: Arc::new(Mutex::new(vec![Ok(ReconcileResult::StillRunning {
                events: Vec::new(),
                backend_state: running_backend_state(),
                status: RunStatus::Running,
            })])),
            reserve_cancel_results: Arc::new(Mutex::new(vec![Ok(CancelReservation {
                backend_state: reserved_cancel_backend_state(1234, true),
                allow_send: true,
            })])),
            cancel_results: Arc::new(Mutex::new(vec![Ok(CancelResult::Reconciling {
                events: Vec::new(),
                backend_state: reserved_cancel_backend_state(1234, true),
            })])),
            ..Default::default()
        };
        let mut first_state = init_state(&root, vec![offer("local-text")], first_adapters.clone());
        let input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"hello"});
        let binding = create_binding("request:cancel-retry", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        run.backend_state = Some(running_backend_state());
        run.status = RunStatus::Running;
        first_state.journal.store_run(&run).unwrap();
        let first_response = first_state
            .handle_runs_cancel(RunsCancelRequest {
                op: "runs_cancel".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        assert_eq!(first_response["data"]["status"], "reconciling");
        let reserved = first_state.journal.load_run(&run.run_id).unwrap();
        assert_eq!(
            reserved.backend_state.as_ref().unwrap()["cancel_deadline_ms"],
            serde_json::json!(1234)
        );

        let retry_adapters = FakeAdapters {
            reconcile_results: Arc::new(Mutex::new(vec![Ok(ReconcileResult::StillRunning {
                events: Vec::new(),
                backend_state: reserved_cancel_backend_state(1234, true),
                status: RunStatus::Reconciling,
            })])),
            reserve_cancel_results: Arc::new(Mutex::new(vec![Ok(CancelReservation {
                backend_state: reserved_cancel_backend_state(1234, true),
                allow_send: false,
            })])),
            cancel_results: Arc::new(Mutex::new(vec![Ok(CancelResult::Reconciling {
                events: Vec::new(),
                backend_state: reserved_cancel_backend_state(1234, true),
            })])),
            ..Default::default()
        };
        let mut retry_state = init_state(&root, vec![offer("local-text")], retry_adapters.clone());
        let response = retry_state
            .handle_runs_cancel(RunsCancelRequest {
                op: "runs_cancel".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        assert_eq!(response["data"]["status"], "reconciling");
        assert_eq!(
            *retry_adapters.cancel_allow_send.lock().unwrap(),
            vec![false]
        );
        let stored = retry_state.journal.load_run(&run.run_id).unwrap();
        assert!(stored.terminal_at_ms.is_none());
        assert_eq!(stored.status, RunStatus::Reconciling);
        assert_eq!(
            stored.backend_state.as_ref().unwrap()["cancel_deadline_ms"],
            serde_json::json!(1234)
        );
    }

    #[test]
    fn later_read_settles_cancelled_status_after_reconciling_cancel() {
        let root = temp_root("cancel-terminal");
        let adapters = FakeAdapters {
            reconcile_results: Arc::new(Mutex::new(vec![
                Ok(ReconcileResult::StillRunning {
                    events: Vec::new(),
                    backend_state: running_backend_state(),
                    status: RunStatus::Running,
                }),
                Ok(ReconcileResult::Terminal {
                    events: Vec::new(),
                    status: RunStatus::Cancelled,
                    output: None,
                    error: Some(RunError {
                        class: ErrorClass::Cancelled,
                        code: "cancelled".to_string(),
                        message: "model run was cancelled".to_string(),
                    }),
                }),
            ])),
            reserve_cancel_results: Arc::new(Mutex::new(vec![Ok(CancelReservation {
                backend_state: reserved_cancel_backend_state(77, false),
                allow_send: false,
            })])),
            cancel_results: Arc::new(Mutex::new(vec![Ok(CancelResult::Reconciling {
                events: Vec::new(),
                backend_state: reserved_cancel_backend_state(77, false),
            })])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"hello"});
        let binding = create_binding("request:cancel-terminal", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding.clone(),
            offer("local-text").summary(),
            offer("local-text").execution_binding_hash().unwrap(),
            now_ms(),
        );
        run.backend_state = Some(running_backend_state());
        run.status = RunStatus::Running;
        state.journal.store_run(&run).unwrap();
        let response = state
            .handle_runs_cancel(RunsCancelRequest {
                op: "runs_cancel".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        assert_eq!(response["data"]["status"], "reconciling");

        let settled = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id,
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        assert_eq!(settled["data"]["status"], "cancelled");
    }

    #[test]
    fn other_run_can_be_read_immediately_after_cancel_enters_reconciling() {
        let root = temp_root("cancel-other-run");
        let adapters = FakeAdapters {
            reconcile_results: Arc::new(Mutex::new(vec![Ok(ReconcileResult::StillRunning {
                events: Vec::new(),
                backend_state: running_backend_state(),
                status: RunStatus::Running,
            })])),
            reserve_cancel_results: Arc::new(Mutex::new(vec![Ok(CancelReservation {
                backend_state: reserved_cancel_backend_state(777, true),
                allow_send: true,
            })])),
            cancel_results: Arc::new(Mutex::new(vec![Ok(CancelResult::Reconciling {
                events: Vec::new(),
                backend_state: reserved_cancel_backend_state(777, true),
            })])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input_one = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"one"});
        let input_two = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"two"});
        let binding_one = create_binding("request:cancel-one", "local-text", &input_one);
        let binding_two = create_binding("request:read-two", "local-text", &input_two);
        let current_offer = offer("local-text");
        let mut run_one = prepared_run_for_offer(binding_one.clone(), &current_offer, now_ms());
        run_one.backend_state = Some(running_backend_state());
        run_one.status = RunStatus::Running;
        let mut run_two = prepared_run_for_offer(binding_two.clone(), &current_offer, now_ms());
        transition_terminal(
            &current_offer,
            &mut run_two,
            RunStatus::Completed,
            Some(serde_json::json!({
                "schema": RUN_OUTPUT_TEXT_SCHEMA,
                "text": "done"
            })),
            None,
        )
        .unwrap();
        state.journal.store_run(&run_one).unwrap();
        state.journal.store_run(&run_two).unwrap();

        let cancel_response = state
            .handle_runs_cancel(RunsCancelRequest {
                op: "runs_cancel".to_string(),
                run_id: run_one.run_id.clone(),
                runtime_binding: access_binding(&binding_one),
            })
            .unwrap();
        let read_response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run_two.run_id.clone(),
                runtime_binding: access_binding(&binding_two),
            })
            .unwrap();

        assert_eq!(cancel_response["data"]["status"], "reconciling");
        assert_eq!(read_response["data"]["status"], "completed");
    }

    #[test]
    fn cancel_deadline_settles_unknown_on_later_read() {
        let root = temp_root("cancel-deadline");
        let deadline = now_ms().saturating_sub(1);
        let adapters = FakeAdapters {
            reconcile_results: Arc::new(Mutex::new(vec![Ok(ReconcileResult::Terminal {
                events: Vec::new(),
                status: RunStatus::SettlementUnknown,
                output: None,
                error: Some(RunError {
                    class: ErrorClass::SettlementUnknown,
                    code: "settlement_unknown".to_string(),
                    message: "model backend settlement is unknown".to_string(),
                }),
            })])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({"schema":"elastos.model.input.text/v1","prompt":"hello"});
        let binding = create_binding("request:cancel-deadline", "local-text", &input);
        let mut run = prepared_run_for_offer(binding.clone(), &offer("local-text"), now_ms());
        run.backend_state = Some(reserved_cancel_backend_state(deadline, true));
        run.status = RunStatus::Reconciling;
        state.journal.store_run(&run).unwrap();

        let response = state
            .handle_runs_get(RunsGetRequest {
                op: "runs_get".to_string(),
                run_id: run.run_id.clone(),
                runtime_binding: access_binding(&binding),
            })
            .unwrap();
        assert_eq!(response["data"]["status"], "settlement_unknown");
    }

    #[test]
    fn nested_runtime_binding_fields_are_rejected() {
        let root = temp_root("nested-input");
        let adapters = FakeAdapters::default();
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello",
            "nested": {
                "items": [
                    {
                        "grant_id": "bad"
                    }
                ]
            }
        });
        let error = state
            .handle_runs_create(RunsCreateRequest {
                op: "runs_create".to_string(),
                offer_id: "local-text".to_string(),
                operation: "text.generate".to_string(),
                input: input.clone(),
                runtime_binding: create_binding("request:nested", "local-text", &input),
            })
            .unwrap_err();
        assert_eq!(error.code(), "invalid_request");
    }

    #[test]
    fn terminal_transition_keeps_exactly_one_terminal_event() {
        let root = temp_root("terminal-once");
        let adapters = FakeAdapters {
            dispatch_results: Arc::new(Mutex::new(vec![Ok(DispatchResult::Terminal {
                events: vec![EventSeed {
                    kind: "progress",
                    data: serde_json::json!({"step":"decode"}),
                }],
                status: RunStatus::Completed,
                output: Some(serde_json::json!({
                    "schema": RUN_OUTPUT_TEXT_SCHEMA,
                    "text": "done"
                })),
                error: None,
            })])),
            ..Default::default()
        };
        let mut state = init_state(&root, vec![offer("local-text")], adapters);
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let response = state
            .handle_runs_create(RunsCreateRequest {
                op: "runs_create".to_string(),
                offer_id: "local-text".to_string(),
                operation: "text.generate".to_string(),
                input: input.clone(),
                runtime_binding: create_binding("request:terminal", "local-text", &input),
            })
            .unwrap();
        let run = state
            .journal
            .load_run(response["data"]["run_id"].as_str().unwrap())
            .unwrap();
        assert_eq!(run.events.iter().filter(|event| event.terminal).count(), 1);
        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.events.last().unwrap().kind, "output");
        assert_eq!(run.events.last().unwrap().data, run.output.clone().unwrap());
    }

    #[test]
    fn adapter_event_seeds_cannot_use_terminal_kinds() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let current_offer = offer("local-text");
        let binding = create_binding("request:reserved-terminal-kind", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let before = serde_json::to_vec(&run).unwrap();

        let error = apply_dispatch_result(
            &current_offer,
            &mut run,
            DispatchResult::Terminal {
                events: vec![EventSeed {
                    kind: "output",
                    data: serde_json::json!({"schema": RUN_OUTPUT_TEXT_SCHEMA, "text": "shadow"}),
                }],
                status: RunStatus::Completed,
                output: Some(serde_json::json!({
                    "schema": RUN_OUTPUT_TEXT_SCHEMA,
                    "text": "done"
                })),
                error: None,
            },
        )
        .unwrap_err();

        assert_eq!(error.code(), "internal_error");
        assert_eq!(serde_json::to_vec(&run).unwrap(), before);
    }

    #[test]
    fn terminal_event_data_matches_canonical_terminal_truth() {
        let cases = [
            (
                RunStatus::Completed,
                Some(serde_json::json!({
                    "schema": RUN_OUTPUT_TEXT_SCHEMA,
                    "text": "done"
                })),
                None,
                "output",
            ),
            (
                RunStatus::Failed,
                None,
                Some(RunError {
                    class: ErrorClass::BackendFailed,
                    code: "backend_failed".to_string(),
                    message: "model backend failed".to_string(),
                }),
                "failed",
            ),
            (
                RunStatus::Cancelled,
                None,
                Some(RunError {
                    class: ErrorClass::Cancelled,
                    code: "cancelled".to_string(),
                    message: "model run was cancelled".to_string(),
                }),
                "cancelled",
            ),
            (
                RunStatus::SettlementUnknown,
                None,
                Some(RunError {
                    class: ErrorClass::SettlementUnknown,
                    code: "settlement_unknown".to_string(),
                    message: "model backend settlement is unknown".to_string(),
                }),
                "settlement_unknown",
            ),
        ];

        for (status, output, error, expected_kind) in cases {
            let input = serde_json::json!({
                "schema": "elastos.model.input.text/v1",
                "prompt": expected_kind
            });
            let current_offer = offer("local-text");
            let binding = create_binding(
                &format!("request:terminal-truth:{expected_kind}"),
                "local-text",
                &input,
            );
            let mut run = StoredRun::new_prepared(
                deterministic_run_id(&binding),
                binding,
                current_offer.summary(),
                current_offer.execution_binding_hash().unwrap(),
                now_ms(),
            );

            transition_terminal(
                &current_offer,
                &mut run,
                status.clone(),
                output.clone(),
                error.clone(),
            )
            .unwrap();

            let terminal_events = run.events.iter().filter(|event| event.terminal).count();
            assert_eq!(terminal_events, 1);
            assert_eq!(run.events.last().unwrap().kind, expected_kind);

            let view = serde_json::to_value(run.to_view()).unwrap();
            match status {
                RunStatus::Completed => {
                    assert_eq!(run.events.last().unwrap().data, run.output.clone().unwrap());
                    assert_eq!(run.events.last().unwrap().data, view["terminal"]["output"]);
                }
                _ => {
                    let expected_error = serde_json::json!({
                        "class": serde_json::to_value(run.error.as_ref().unwrap().class.clone()).unwrap(),
                        "code": run.error.as_ref().unwrap().code,
                        "message": run.error.as_ref().unwrap().message,
                    });
                    assert_eq!(run.events.last().unwrap().data, expected_error);
                    assert_eq!(run.events.last().unwrap().data, view["terminal"]["error"]);
                }
            }
        }
    }

    #[test]
    fn terminal_event_limit_failure_leaves_run_byte_identical() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:terminal-event-limit", "local-text", &input);
        let mut current_offer = offer("local-text");
        current_offer.policy.event_bytes_limit = 96;
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let before = serde_json::to_vec(&run).unwrap();

        let error = transition_terminal(
            &current_offer,
            &mut run,
            RunStatus::Completed,
            Some(output_text_bytes(128)),
            None,
        )
        .unwrap_err();

        assert_eq!(error.code(), "policy_limit");
        assert_eq!(serde_json::to_vec(&run).unwrap(), before);
    }

    #[test]
    fn dispatch_terminal_result_is_atomic_when_terminal_payload_is_invalid() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let current_offer = offer("local-text");
        let binding = create_binding("request:atomic-dispatch-terminal", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let before = serde_json::to_vec(&run).unwrap();

        let error = apply_dispatch_result(
            &current_offer,
            &mut run,
            DispatchResult::Terminal {
                events: vec![EventSeed {
                    kind: "progress",
                    data: serde_json::json!({"step": "decode"}),
                }],
                status: RunStatus::Completed,
                output: None,
                error: None,
            },
        )
        .unwrap_err();

        assert_eq!(error.code(), "internal_error");
        assert_eq!(serde_json::to_vec(&run).unwrap(), before);
    }

    #[test]
    fn reconcile_result_is_atomic_when_later_event_exceeds_bounds() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let binding = create_binding("request:atomic-reconcile-events", "local-text", &input);
        let mut current_offer = offer("local-text");
        current_offer.policy.event_bytes_limit = 256;
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let before = serde_json::to_vec(&run).unwrap();

        let error = apply_reconcile_result(
            &current_offer,
            &mut run,
            ReconcileResult::StillRunning {
                events: vec![
                    EventSeed {
                        kind: "progress",
                        data: serde_json::json!({"step": "accepted"}),
                    },
                    EventSeed {
                        kind: "progress",
                        data: event_data_bytes(4 * 1024),
                    },
                ],
                backend_state: running_backend_state(),
                status: RunStatus::Running,
            },
        )
        .unwrap_err();

        assert_eq!(error.code(), "policy_limit");
        assert_eq!(serde_json::to_vec(&run).unwrap(), before);
    }

    #[test]
    fn cancel_terminal_result_is_atomic_when_terminal_payload_is_invalid() {
        let input = serde_json::json!({
            "schema": "elastos.model.input.text/v1",
            "prompt": "hello"
        });
        let current_offer = offer("local-text");
        let binding = create_binding("request:atomic-cancel-terminal", "local-text", &input);
        let mut run = StoredRun::new_prepared(
            deterministic_run_id(&binding),
            binding,
            current_offer.summary(),
            current_offer.execution_binding_hash().unwrap(),
            now_ms(),
        );
        let before = serde_json::to_vec(&run).unwrap();

        let error = apply_cancel_result(
            &current_offer,
            &mut run,
            CancelResult::Terminal {
                events: vec![EventSeed {
                    kind: "progress",
                    data: serde_json::json!({"step": "cancelled"}),
                }],
                status: RunStatus::Failed,
                output: None,
                error: None,
            },
        )
        .unwrap_err();

        assert_eq!(error.code(), "internal_error");
        assert_eq!(serde_json::to_vec(&run).unwrap(), before);
    }
}

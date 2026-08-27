use super::*;

#[derive(Clone)]
struct RecordedDirectCarrierCall {
    peer_did: String,
    source: String,
    target: String,
    op: String,
    request: serde_json::Value,
}

struct RecordingDirectCarrierInvoker {
    source_endpoint_did: String,
    remote: CollaborationDirectMessageService,
    calls: tokio::sync::Mutex<Vec<RecordedDirectCarrierCall>>,
}

#[async_trait::async_trait]
impl elastos_runtime::provider::ProviderCarrierInvoker for RecordingDirectCarrierInvoker {
    async fn invoke_carrier_provider(
        &self,
        route: &elastos_runtime::provider::ProviderCarrierRoute,
        invocation: &elastos_runtime::provider::ProviderInvocation,
        request: serde_json::Value,
    ) -> Result<serde_json::Value, elastos_runtime::provider::ProviderError> {
        let peer_did = match route {
            elastos_runtime::provider::ProviderCarrierRoute::PeerDid { peer_did, .. } => {
                peer_did.clone()
            }
            _ => {
                return Err(elastos_runtime::provider::ProviderError::Provider(
                    "direct delivery must stay on the peer-did route".to_string(),
                ));
            }
        };
        self.calls.lock().await.push(RecordedDirectCarrierCall {
            peer_did,
            source: invocation.source.clone(),
            target: invocation.target.clone(),
            op: invocation.op.clone(),
            request: request.clone(),
        });
        let message = request
            .get("message")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                elastos_runtime::provider::ProviderError::Provider(
                    "direct delivery request is missing its message".to_string(),
                )
            })?;
        let envelope = decode(message, "direct message").map_err(|err| {
            elastos_runtime::provider::ProviderError::Provider(format!(
                "direct delivery request is invalid: {err}"
            ))
        })?;
        let receipt = self
            .remote
            .receive(&envelope, &self.source_endpoint_did, now_ts())
            .map_err(|err| {
                elastos_runtime::provider::ProviderError::Provider(format!(
                    "direct delivery receive failed: {err}"
                ))
            })?;
        Ok(serde_json::json!({
            "status": "ok",
            "receipt": encode(&receipt),
        }))
    }
}

fn fixture_source_endpoint(message: &[u8]) -> String {
    serde_json::from_slice::<SignedCollaborationMessage>(message)
        .unwrap()
        .signer_did
}

fn test_message(
    pair: &crate::collaboration_discovery_runtime::tests::DirectPeerPair,
    request_id: &str,
    conversation_id: &str,
    recipient_profile_did: &str,
    text: &str,
    now: u64,
) -> Vec<u8> {
    prepare_direct_message(
        &pair.key_a,
        &pair.service_a.network_profile(),
        &pair.profile_a,
        DirectMessageIntent {
            request_id,
            conversation_id,
            recipient_profile_did,
            text,
        },
        now,
    )
    .unwrap()
}

fn resign_message(
    bytes: &[u8],
    signing_key: &SigningKey,
    mutate: impl FnOnce(&mut CollaborationMessage),
) -> Vec<u8> {
    let mut envelope: SignedCollaborationMessage = serde_json::from_slice(bytes).unwrap();
    mutate(&mut envelope.payload);
    let (signature, signer_did) = domain_separated_sign(
        signing_key,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
        &canonical_collaboration_message_bytes(&envelope.payload).unwrap(),
    );
    envelope.signature = signature;
    envelope.signer_did = signer_did;
    canonical_signed_collaboration_message_bytes(&envelope).unwrap()
}

fn protected_store_bytes(store: &DirectMessageStore) -> Option<Vec<u8>> {
    match std::fs::read(store.object_path().unwrap()) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("failed to read protected message store: {error}"),
    }
}

fn clear_message_store(store: &DirectMessageStore) {
    store
        .mutate(|state| {
            state.messages.clear();
            state.receipts.clear();
            Ok(())
        })
        .unwrap();
}

fn persist_terminal_outgoing(
    pair: &crate::collaboration_discovery_runtime::tests::DirectPeerPair,
    store: &DirectMessageStore,
    request_id: &str,
    text: &str,
    created_at: u64,
) -> Vec<u8> {
    let remote_did = pair.store_a.snapshot().unwrap().contacts()[0]
        .remote_profile_did()
        .to_string();
    let message = test_message(
        pair,
        request_id,
        &pair.conversation_id,
        &remote_did,
        text,
        created_at,
    );
    store.persist_message(&message, false, created_at).unwrap();
    let receipt = pair
        .service_b
        .direct_message_service()
        .receive(&message, &fixture_source_endpoint(&message), created_at)
        .unwrap();
    store
        .persist_receipt(
            &collaboration_message_envelope_sha256(&message),
            &receipt,
            created_at,
        )
        .unwrap();
    message
}

#[tokio::test]
async fn test_context_authority_is_explicit_and_never_selected_by_string_values() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let direct = pair.service_a.direct_message_service();
    let profile_did = pair.profile_a.document().profile_did.clone();
    {
        let mut contexts = direct.inner.contexts.lock().unwrap();
        contexts.get_mut(&profile_did).unwrap().authority = DirectContextAuthority::Session {
            session_id: "ordinary-session-value".to_string(),
            proof_binding_id: None,
            grant_id: "ordinary-grant-value".to_string(),
        };
    }
    assert!(direct.context(&profile_did, now_ts()).is_err());
}

#[tokio::test]
async fn retention_prunes_only_oldest_expired_terminal_pairs_and_preserves_state_on_exhaustion() {
    assert_eq!(MAX_DIRECT_MESSAGE_STATE_BYTES, 24 * 1024 * 1024);
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let direct = pair.service_a.direct_message_service();
    let context = direct
        .context(&pair.profile_a.document().profile_did, now_ts())
        .unwrap();
    let store = context.message_store.as_ref();
    let now = now_ts();
    let old = now - DIRECT_MESSAGE_TTL_SECS - MAX_COLLABORATION_CLOCK_SKEW_SECS - 100;
    let oldest = persist_terminal_outgoing(&pair, store, "oldest-terminal", "oldest", old);
    let newer = persist_terminal_outgoing(&pair, store, "newer-terminal", "newerxx", old + 1);
    let remote_did = pair.store_a.snapshot().unwrap().contacts()[0]
        .remote_profile_did()
        .to_string();
    let pending = test_message(
        &pair,
        "entry-pressure",
        &pair.conversation_id,
        &remote_did,
        "pending",
        now,
    );
    store
        .persist_message_with_limits(&pending, false, now, 2, MAX_DIRECT_MESSAGE_STATE_BYTES)
        .unwrap();
    let state = store.load().unwrap().unwrap();
    let envelopes = state
        .messages
        .iter()
        .map(|entry| decode(&entry.envelope, "direct message").unwrap())
        .collect::<Vec<_>>();
    assert!(!envelopes.contains(&oldest));
    assert!(envelopes.contains(&newer));
    assert!(envelopes.contains(&pending));
    assert_eq!(state.receipts.len(), 1);
    assert_eq!(
        state.receipts[0].message_envelope_sha256,
        collaboration_message_envelope_sha256(&newer)
    );

    clear_message_store(store);
    let byte_terminal = persist_terminal_outgoing(&pair, store, "byte-terminal", "terminal", old);
    let byte_pending = test_message(
        &pair,
        "byte-pressure",
        &pair.conversation_id,
        &remote_did,
        "pendingx",
        now,
    );
    let mut expected = store.load().unwrap().unwrap();
    expected.messages.push(StoredMessage {
        envelope: encode(&byte_pending),
        incoming: false,
        recorded_at: now,
    });
    expected
        .messages
        .retain(|entry| decode(&entry.envelope, "direct message").unwrap() != byte_terminal);
    expected.receipts.clear();
    let byte_limit = serde_json::to_vec(&expected).unwrap().len();
    store
        .persist_message_with_limits(&byte_pending, false, now, MAX_DIRECT_MESSAGES, byte_limit)
        .unwrap();
    let byte_state = store.load().unwrap().unwrap();
    assert_eq!(byte_state.messages.len(), 1);
    assert!(byte_state.receipts.is_empty());
    assert_eq!(
        decode(&byte_state.messages[0].envelope, "direct message").unwrap(),
        byte_pending
    );

    clear_message_store(store);
    // An abandoned record — expired, no receipt, the shape a peer that never
    // acknowledges produces — is terminal and must yield to a live write.
    // Before, it was unprunable and one dead contact could wedge the store.
    let expired_pending = test_message(
        &pair,
        "expired-pending",
        &pair.conversation_id,
        &remote_did,
        "expired",
        old,
    );
    store.persist_message(&expired_pending, false, old).unwrap();
    let unexpired_pending = test_message(
        &pair,
        "unexpired-pending",
        &pair.conversation_id,
        &remote_did,
        "unexpired",
        now,
    );
    store
        .persist_message(&unexpired_pending, false, now)
        .unwrap();
    let accepted = test_message(
        &pair,
        "capacity-accepted",
        &pair.conversation_id,
        &remote_did,
        "accepted",
        now,
    );
    store
        .persist_message_with_limits(&accepted, false, now, 2, MAX_DIRECT_MESSAGE_STATE_BYTES)
        .unwrap();
    let retained = store.records().unwrap();
    assert!(!retained
        .iter()
        .any(|record| record.envelope_bytes == expired_pending));
    assert!(retained
        .iter()
        .any(|record| record.envelope_bytes == unexpired_pending));
    assert!(retained
        .iter()
        .any(|record| record.envelope_bytes == accepted));

    // With every record live — unexpired, still being retried — refusing the
    // write is honest backpressure and the store is untouched.
    let before = protected_store_bytes(store);
    let rejected = test_message(
        &pair,
        "capacity-rejected",
        &pair.conversation_id,
        &remote_did,
        "rejected",
        now,
    );
    assert!(store
        .persist_message_with_limits(&rejected, false, now, 2, MAX_DIRECT_MESSAGE_STATE_BYTES,)
        .is_err());
    assert_eq!(protected_store_bytes(store), before);

    clear_message_store(store);
    let receipt_pressure = test_message(
        &pair,
        "receipt-byte-pressure",
        &pair.conversation_id,
        &remote_did,
        "terminal",
        old,
    );
    store
        .persist_message(&receipt_pressure, false, old)
        .unwrap();
    let receipt = pair
        .service_b
        .direct_message_service()
        .receive(
            &receipt_pressure,
            &fixture_source_endpoint(&receipt_pressure),
            old,
        )
        .unwrap();
    let mut empty = store.load().unwrap().unwrap();
    empty.messages.clear();
    empty.receipts.clear();
    let empty_bytes = serde_json::to_vec(&empty).unwrap().len();
    store
        .persist_receipt_with_limits(
            &collaboration_message_envelope_sha256(&receipt_pressure),
            &receipt,
            now,
            MAX_DIRECT_MESSAGES,
            empty_bytes,
        )
        .unwrap();
    let settled = store.load().unwrap().unwrap();
    assert!(settled.messages.is_empty());
    assert!(settled.receipts.is_empty());
}

#[tokio::test]
async fn production_entry_limit_prunes_one_expired_terminal_pair_before_write() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let direct = pair.service_a.direct_message_service();
    let context = direct
        .context(&pair.profile_a.document().profile_did, now_ts())
        .unwrap();
    let store = context.message_store.as_ref();
    let now = now_ts();
    let old = now - DIRECT_MESSAGE_TTL_SECS - MAX_COLLABORATION_CLOCK_SKEW_SECS - 100;
    let remote_did = pair.store_a.snapshot().unwrap().contacts()[0]
        .remote_profile_did()
        .to_string();
    let terminal = test_message(
        &pair,
        "production-terminal",
        &pair.conversation_id,
        &remote_did,
        "terminal",
        old,
    );
    let terminal_receipt = pair
        .service_b
        .direct_message_service()
        .receive(&terminal, &fixture_source_endpoint(&terminal), old)
        .unwrap();
    let mut state = DirectMessageState {
        schema: DIRECT_MESSAGE_STATE_SCHEMA.to_string(),
        network_id: pair
            .service_a
            .network_profile()
            .profile()
            .network_id
            .clone(),
        local_profile_did: pair.profile_a.document().profile_did.clone(),
        messages: vec![StoredMessage {
            envelope: encode(&terminal),
            incoming: false,
            recorded_at: old,
        }],
        receipts: vec![StoredReceipt {
            message_envelope_sha256: collaboration_message_envelope_sha256(&terminal),
            envelope: encode(&terminal_receipt),
        }],
    };
    for index in 0..MAX_DIRECT_MESSAGES - 1 {
        let message = test_message(
            &pair,
            &format!("production-pending-{index:04}"),
            &pair.conversation_id,
            &remote_did,
            "pending",
            now,
        );
        state.messages.push(StoredMessage {
            envelope: encode(&message),
            incoming: false,
            recorded_at: now,
        });
    }
    let bytes = serde_json::to_vec(&state).unwrap();
    assert!(bytes.len() < MAX_DIRECT_MESSAGE_STATE_BYTES);
    write_protected_principal_root_object(
        &store.data_root,
        &store.principal_id,
        &store.localhost_root,
        &store.object_uri(),
        &store.object_path().unwrap(),
        &bytes,
    )
    .unwrap();
    let newest = test_message(
        &pair,
        "production-newest",
        &pair.conversation_id,
        &remote_did,
        "newest",
        now,
    );
    store.persist_message(&newest, false, now).unwrap();
    let records = store.records().unwrap();
    assert_eq!(records.len(), MAX_DIRECT_MESSAGES);
    assert!(!records
        .iter()
        .any(|record| record.envelope_bytes == terminal));
    assert!(records.iter().any(|record| record.envelope_bytes == newest));
    assert!(!store
        .has_receipt(&collaboration_message_envelope_sha256(&terminal))
        .unwrap());
}

#[tokio::test]
async fn renaming_yourself_does_not_stop_your_contacts_reaching_you() {
    // A person's identity is their Profile DID, and renaming does not move
    // it — only the revision advances. A context registered before the
    // rename must keep accepting mail, because nothing about who anyone is
    // has changed. This used to fail: the check compared whole signed
    // Profile documents, so a new name read as a new authority.
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let b_proof_binding =
        crate::collaboration_discovery_runtime::tests::fixture_passkey_proof_binding(
            pair.store_b.data_root(),
            pair.store_b.principal_id(),
            "runtime-owned-b",
        );
    let now = now_ts();
    let b_device = pair.store_a.snapshot().unwrap().contacts()[0]
        .remote_profile_did()
        .to_string();

    let renamed_b = crate::collaboration_discovery_runtime::tests::renamed_discovery_profile(
        &pair.key_b,
        "Bruno With A New Name",
        Some("bob"),
        &pair.profile_b,
    );
    assert_eq!(
        renamed_b.document().profile_did,
        pair.profile_b.document().profile_did,
        "a rename must not move the identity"
    );

    pair.registry_b
        .unregister_sub_provider(DIRECT_MESSAGE_PROVIDER_SCHEME)
        .await
        .unwrap();
    let receiver = CollaborationDirectMessageService::new(
        SigningKey::from_bytes(&pair.key_b.to_bytes()),
        pair.service_b.network_profile(),
        pair.registry_b.clone(),
    )
    .await
    .unwrap();
    // Registered under the pre-rename Profile, then handed the rename.
    receiver
        .register_runtime_owned_context(
            pair.store_b.clone(),
            pair.profile_b.clone(),
            &b_proof_binding,
        )
        .unwrap();
    receiver
        .register_runtime_owned_context(pair.store_b.clone(), renamed_b.clone(), &b_proof_binding)
        .expect("a newer revision of the same DID is the same person");

    let message = test_message(
        &pair,
        "rename-does-not-break-delivery",
        &pair.conversation_id,
        &b_device,
        "still reachable after renaming",
        now,
    );
    let receipt = receiver
        .receive(&message, &fixture_source_endpoint(&message), now)
        .expect("renaming yourself must not refuse your contacts' messages");
    assert!(!receipt.is_empty());
}

#[tokio::test]
async fn a_running_home_receives_without_a_signed_in_session() {
    // A Home that is running is reachable. Delivery contexts used to exist
    // only while a signed-in browser session held one, so a running Home
    // with nobody looking at it refused messages from contacts it had
    // already accepted. Here the receiving service is built fresh — as a
    // restarted Runtime is, with no session anywhere — and registers the
    // way startup does.
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let b_proof_binding =
        crate::collaboration_discovery_runtime::tests::fixture_passkey_proof_binding(
            pair.store_b.data_root(),
            pair.store_b.principal_id(),
            "runtime-owned-b",
        );
    let now = now_ts();
    let b_device = pair.store_a.snapshot().unwrap().contacts()[0]
        .remote_profile_did()
        .to_string();

    pair.registry_b
        .unregister_sub_provider(DIRECT_MESSAGE_PROVIDER_SCHEME)
        .await
        .unwrap();
    let restarted_b = CollaborationDirectMessageService::new(
        SigningKey::from_bytes(&pair.key_b.to_bytes()),
        pair.service_b.network_profile(),
        pair.registry_b.clone(),
    )
    .await
    .unwrap();

    let message = test_message(
        &pair,
        "no-session-delivery",
        &pair.conversation_id,
        &b_device,
        "reachable because it is running",
        now,
    );

    // Before startup registration there is no recipient at all.
    assert!(restarted_b
        .receive(&message, &fixture_source_endpoint(&message), now)
        .is_err());

    restarted_b
        .register_runtime_owned_context(
            pair.store_b.clone(),
            pair.profile_b.clone(),
            &b_proof_binding,
        )
        .unwrap();

    // A verified envelope from an accepted contact is now accepted, and the
    // sender gets the receipt that settles its send.
    let receipt = restarted_b
        .receive(&message, &fixture_source_endpoint(&message), now)
        .expect("a running Home accepts its owner's mail without a browser session");
    assert!(!receipt.is_empty());
    let stored = restarted_b
        .context(&pair.profile_b.document().profile_did, now)
        .unwrap()
        .message_store
        .records()
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert!(stored[0].incoming);
}

#[tokio::test]
async fn revoking_the_passkey_stops_the_running_home_receiving() {
    // Authority that outlives every session has to be endable by something,
    // or a stolen laptop keeps taking mail after its owner has revoked the
    // passkey and watched every browser lose access. A Runtime-owned
    // context is registered against a proof binding, and revoking that
    // binding must close it as surely as ending a session closes a
    // session-owned one.
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let b_proof_binding =
        crate::collaboration_discovery_runtime::tests::fixture_passkey_proof_binding(
            pair.store_b.data_root(),
            pair.store_b.principal_id(),
            "runtime-owned-b",
        );
    let now = now_ts();
    let b_device = pair.store_a.snapshot().unwrap().contacts()[0]
        .remote_profile_did()
        .to_string();

    pair.registry_b
        .unregister_sub_provider(DIRECT_MESSAGE_PROVIDER_SCHEME)
        .await
        .unwrap();
    let running_b = CollaborationDirectMessageService::new(
        SigningKey::from_bytes(&pair.key_b.to_bytes()),
        pair.service_b.network_profile(),
        pair.registry_b.clone(),
    )
    .await
    .unwrap();
    running_b
        .register_runtime_owned_context(
            pair.store_b.clone(),
            pair.profile_b.clone(),
            &b_proof_binding,
        )
        .unwrap();

    let before = test_message(
        &pair,
        "accepted-before-revocation",
        &pair.conversation_id,
        &b_device,
        "arrives while the passkey is live",
        now,
    );
    running_b
        .receive(&before, &fixture_source_endpoint(&before), now)
        .expect("a live passkey receives");
    // Held before revocation, because afterwards even looking the context
    // up is refused — which is itself the point.
    let message_store = running_b
        .context(&pair.profile_b.document().profile_did, now)
        .unwrap()
        .message_store
        .clone();

    crate::auth::revoke_passkey_binding(pair.store_b.data_root(), &b_proof_binding, now).unwrap();

    // Same accepted contact, same envelope shape, same running process —
    // and now refused, with nothing restarted.
    let after = test_message(
        &pair,
        "refused-after-revocation",
        &pair.conversation_id,
        &b_device,
        "must not arrive once the passkey is revoked",
        now,
    );
    let refused = running_b
        .receive(&after, &fixture_source_endpoint(&after), now)
        .expect_err("a revoked passkey stops the Runtime receiving for that person")
        .to_string();
    assert!(
        refused.contains("revoked"),
        "the refusal should name revocation, said: {refused}"
    );

    assert_eq!(
        message_store.records().unwrap().len(),
        1,
        "the refused message must not be persisted beside the accepted one"
    );
}

#[tokio::test]
async fn incoming_direct_message_notifies_until_the_conversation_is_read() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let now = now_ts();
    let direct_b = pair.service_b.direct_message_service();
    let b_root = pair.store_b.data_root().to_path_buf();
    let b_device = pair.store_a.snapshot().unwrap().contacts()[0]
        .remote_profile_did()
        .to_string();

    let message = test_message(
        &pair,
        "notify-first",
        &pair.conversation_id,
        &b_device,
        "hello there",
        now,
    );
    direct_b
        .receive(&message, &fixture_source_endpoint(&message), now)
        .unwrap();

    // A verified incoming message tells the person, named by the signed
    // contact presentation, pointing at Chat — never carrying the message
    // decision surface itself.
    let summary = crate::notifications::load_summary(&b_root).unwrap();
    assert_eq!(summary.entries.len(), 1);
    assert_eq!(summary.entries[0].kind, "direct_message");
    assert_eq!(summary.entries[0].title, "New message from Alice");
    assert_eq!(summary.entries[0].source_app, "chat-room");
    let action_id =
        crate::notifications::direct_message_notification_action_id(&pair.conversation_id);
    assert_eq!(
        summary.entries[0]
            .action_ref
            .as_ref()
            .map(|action| action.action_id.as_str()),
        Some(action_id.as_str())
    );

    // Reading the conversation resolves the notification, and an idempotent
    // replay of the same envelope returns the stored receipt without
    // resurfacing it.
    assert_eq!(
        crate::notifications::mark_acted_for_action(&b_root, &action_id).unwrap(),
        1
    );
    assert!(crate::notifications::load_summary(&b_root)
        .unwrap()
        .entries
        .is_empty());
    direct_b
        .receive(&message, &fixture_source_endpoint(&message), now + 1)
        .unwrap();
    assert!(crate::notifications::load_summary(&b_root)
        .unwrap()
        .entries
        .is_empty());

    // A genuinely new message resurfaces it.
    let second = test_message(
        &pair,
        "notify-second",
        &pair.conversation_id,
        &b_device,
        "are you there",
        now + 2,
    );
    direct_b
        .receive(&second, &fixture_source_endpoint(&second), now + 2)
        .unwrap();
    let resurfaced = crate::notifications::load_summary(&b_root).unwrap();
    assert_eq!(resurfaced.entries.len(), 1);
    assert!(!resurfaced.entries[0].read);
}

#[tokio::test]
async fn bilateral_removal_delivers_the_signed_revocation_and_both_sides_stay_visible() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let now = now_ts();
    let a_did = pair.profile_a.document().profile_did.clone();
    let b_did = pair.profile_b.document().profile_did.clone();

    // A removes B: immediate locally, durable revocation queued.
    pair.service_a
        .remove_contact(&pair.store_a, &pair.profile_a, &b_did, now)
        .await
        .unwrap();
    let a_snapshot = pair.store_a.snapshot().unwrap();
    assert!(a_snapshot.contacts().is_empty());
    assert_eq!(a_snapshot.removed().len(), 1);
    assert!(a_snapshot.removed()[0].removed_by_local());
    let resendable = pair.store_a.resendable_contact_revocations(8).unwrap();
    assert_eq!(resendable.len(), 1);

    // Sending to the removed pair is forbidden as of the same write.
    let direct_a = pair.service_a.direct_message_service();
    assert!(matches!(
        direct_a
            .send_text(&a_did, "post-remove", &pair.conversation_id, "blocked", now)
            .await,
        Err(DirectApiError::ForbiddenConversation)
    ));

    // The exact revocation delivers to B's device and B acknowledges.
    direct_a
        .deliver_contact_revocation(
            &resendable[0].envelope,
            &resendable[0].recipient_endpoint_did,
            now,
        )
        .await
        .unwrap();
    pair.store_a
        .settle_local_contact_revocation(&b_did, now)
        .unwrap();
    assert!(pair
        .store_a
        .resendable_contact_revocations(8)
        .unwrap()
        .is_empty());

    // Removal is symmetric and visible: B keeps the pair as removed-by-them,
    // named by the signed presentation, not vanished.
    let b_snapshot = pair.store_b.snapshot().unwrap();
    assert!(b_snapshot.contacts().is_empty());
    assert_eq!(b_snapshot.removed().len(), 1);
    assert!(!b_snapshot.removed()[0].removed_by_local());
    assert_eq!(b_snapshot.removed()[0].remote_profile_did(), a_did);

    // Idempotent: a duplicate delivery still settles the sender's retry.
    direct_a
        .deliver_contact_revocation(
            &resendable[0].envelope,
            &resendable[0].recipient_endpoint_did,
            now,
        )
        .await
        .unwrap();

    // B can no longer message A either — removed permits reading only.
    let direct_b = pair.service_b.direct_message_service();
    assert!(matches!(
        direct_b
            .send_text(
                &b_did,
                "b-post-remove",
                &pair.conversation_id,
                "blocked",
                now
            )
            .await,
        Err(DirectApiError::ForbiddenConversation)
    ));
}

#[tokio::test]
async fn abandoned_message_reads_expired_never_pending_and_a_receipt_still_wins() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    // The recipient never comes back: the envelope stays durable and unsettled.
    pair._node_b.endpoint.close().await;

    let direct_a = pair.service_a.direct_message_service();
    let sent_at = now_ts();
    assert_eq!(
        direct_a
            .send_text(
                &pair.profile_a.document().profile_did,
                "expiry-request",
                &pair.conversation_id,
                "abandoned at ttl",
                sent_at,
            )
            .await
            .unwrap(),
        DirectDeliveryStatus::Pending,
    );

    // While the TTL runs the message is honestly still being retried.
    let live = direct_a
        .message_summaries(
            pair.store_a.as_ref(),
            &pair.profile_a,
            &pair.conversation_id,
            sent_at,
        )
        .unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].delivery_state, "pending");

    // One second past the TTL the Runtime has stopped retrying
    // (retry_pending skips expired envelopes), so the read model must say
    // so rather than keep reporting a send that will never happen.
    let past_ttl = sent_at + super::DIRECT_MESSAGE_TTL_SECS + 1;
    let abandoned = direct_a
        .message_summaries(
            pair.store_a.as_ref(),
            &pair.profile_a,
            &pair.conversation_id,
            past_ttl,
        )
        .unwrap();
    assert_eq!(abandoned.len(), 1);
    assert_eq!(abandoned[0].delivery_state, "expired");
    assert_eq!(abandoned[0].direction, "outgoing");

    // A settled receipt is stronger than expiry: a message the recipient
    // acknowledged stays receipt_settled forever, even read after the TTL.
    // The receipt is the real signed artifact, built and persisted exactly as
    // the receive path would: verified message in, recipient-signed receipt out.
    let context = direct_a
        .context(&pair.profile_a.document().profile_did, now_ts())
        .unwrap();
    let records = context.message_store.records().unwrap();
    assert_eq!(records.len(), 1);
    let verified = super::verify_direct_message(
        &records[0].envelope_bytes,
        &pair.service_a.network_profile(),
        &pair.conversation_id,
        &pair.profile_a,
        &pair.profile_b,
        sent_at,
    )
    .unwrap();
    let receipt = super::acceptance_receipt_for(&pair.key_b, &verified, sent_at + 1).unwrap();
    context
        .message_store
        .persist_receipt(verified.envelope_sha256(), &receipt, sent_at + 1)
        .unwrap();
    let settled = direct_a
        .message_summaries(
            pair.store_a.as_ref(),
            &pair.profile_a,
            &pair.conversation_id,
            past_ttl,
        )
        .unwrap();
    assert_eq!(settled[0].delivery_state, "receipt_settled");
}

#[tokio::test]
async fn durable_pending_restarts_with_the_exact_envelope_and_settles_once() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    pair._node_b.endpoint.close().await;

    let direct_a = pair.service_a.direct_message_service();
    assert_eq!(
        direct_a
            .send_text(
                &pair.profile_a.document().profile_did,
                "restart-request",
                &pair.conversation_id,
                "survives restart",
                now_ts(),
            )
            .await
            .unwrap(),
        DirectDeliveryStatus::Pending,
    );
    let before = direct_a
        .context(&pair.profile_a.document().profile_did, now_ts())
        .unwrap()
        .message_store
        .records()
        .unwrap();
    assert_eq!(before.len(), 1);
    assert!(!before[0].incoming);
    assert!(!before[0].receipt_settled);
    let exact_envelope = before[0].envelope_bytes.clone();
    let exact_message: SignedCollaborationMessage =
        serde_json::from_slice(&exact_envelope).unwrap();

    pair.registry_a
        .unregister_sub_provider(DIRECT_MESSAGE_PROVIDER_SCHEME)
        .await
        .unwrap();
    let restarted = CollaborationDirectMessageService::new(
        SigningKey::from_bytes(&pair.key_a.to_bytes()),
        pair.service_a.network_profile(),
        pair.registry_a.clone(),
    )
    .await
    .unwrap();
    restarted
        .register_verified_context_for_test(pair.store_a.clone(), pair.profile_a.clone())
        .unwrap();
    let restarted_b = crate::carrier::start_carrier_node_with_registry(
        &pair.key_b,
        &crate::crypto::encode_signing_key_did(&pair.key_b),
        temp.path().join("b-restarted"),
        Some(Arc::downgrade(&pair.registry_b)),
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    restarted
        .retry_pending(&pair.profile_a.document().profile_did, now_ts())
        .await
        .unwrap();
    let after = restarted
        .context(&pair.profile_a.document().profile_did, now_ts())
        .unwrap()
        .message_store
        .records()
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].envelope_bytes, exact_envelope);
    assert!(after[0].receipt_settled);
    let after_message: SignedCollaborationMessage =
        serde_json::from_slice(&after[0].envelope_bytes).unwrap();
    assert_eq!(
        after_message.payload.message_id,
        exact_message.payload.message_id
    );
    assert_eq!(after_message.payload.nonce, exact_message.payload.nonce);
    assert_eq!(
        serde_json::from_value::<DirectMessagePayload>(after_message.payload.payload)
            .unwrap()
            .request_id,
        "restart-request"
    );
    let remote = pair
        .service_b
        .direct_message_service()
        .context(&pair.profile_b.document().profile_did, now_ts())
        .unwrap()
        .message_store
        .records()
        .unwrap();
    assert_eq!(remote.iter().filter(|record| record.incoming).count(), 1);
    drop(restarted_b);
}

#[tokio::test]
async fn retry_filters_invalid_and_removed_records_before_its_bounded_budget() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let direct_a = pair.service_a.direct_message_service();
    let context = direct_a
        .context(&pair.profile_a.document().profile_did, now_ts())
        .unwrap();
    let now = now_ts();
    let remote_did = pair.store_a.snapshot().unwrap().contacts()[0]
        .remote_profile_did()
        .to_string();

    let expired = test_message(
        &pair,
        "expired-request",
        &pair.conversation_id,
        &remote_did,
        "expired",
        now - DIRECT_MESSAGE_TTL_SECS - 1,
    );
    context
        .message_store
        .persist_message(&expired, false, now)
        .unwrap();
    let future = test_message(
        &pair,
        "future-request",
        &pair.conversation_id,
        &remote_did,
        "future",
        now + 31,
    );
    context
        .message_store
        .persist_message(&future, false, now)
        .unwrap();

    let removed_key =
        SigningKey::from_bytes(&elastos_runtime::signature::generate_keypair().0.to_bytes());
    let removed_did = crate::crypto::encode_signing_key_did(&removed_key);
    for index in 0..4 {
        let removed = test_message(
            &pair,
            &format!("removed-request-{index}"),
            &format!("removed-conversation-{index}"),
            &removed_did,
            "removed",
            now,
        );
        context
            .message_store
            .persist_message(&removed, false, now)
            .unwrap();
    }
    let valid = test_message(
        &pair,
        "eligible-request",
        &pair.conversation_id,
        &remote_did,
        "eligible",
        now,
    );
    context
        .message_store
        .persist_message(&valid, false, now)
        .unwrap();

    direct_a
        .retry_pending(&pair.profile_a.document().profile_did, now)
        .await
        .unwrap();
    let after_first = context.message_store.records().unwrap();
    assert_eq!(after_first.len(), 7);
    assert_eq!(
        after_first
            .iter()
            .filter(|record| record.receipt_settled)
            .count(),
        1
    );
    assert!(
        after_first
            .iter()
            .find(|record| record.envelope_bytes == valid)
            .unwrap()
            .receipt_settled
    );
    assert!(after_first
        .iter()
        .filter(|record| record.envelope_bytes != valid)
        .all(|record| !record.receipt_settled));
    let remote_after_first = pair
        .service_b
        .direct_message_service()
        .context(&pair.profile_b.document().profile_did, now_ts())
        .unwrap()
        .message_store
        .records()
        .unwrap();
    assert_eq!(
        remote_after_first
            .iter()
            .filter(|record| record.incoming)
            .count(),
        1
    );

    direct_a
        .retry_pending(&pair.profile_a.document().profile_did, now)
        .await
        .unwrap();
    assert_eq!(context.message_store.records().unwrap(), after_first);
    assert_eq!(
        pair.service_b
            .direct_message_service()
            .context(&pair.profile_b.document().profile_did, now_ts())
            .unwrap()
            .message_store
            .records()
            .unwrap(),
        remote_after_first
    );
}

#[tokio::test]
async fn unavailable_first_contact_does_not_starve_later_reachable_pending_message() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let (offline_did, offline_conversation) =
        crate::collaboration_discovery_runtime::tests::add_offline_accepted_contact(&pair);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let direct = pair.service_a.direct_message_service();
    let context = direct
        .context(&pair.profile_a.document().profile_did, now_ts())
        .unwrap();
    let now = now_ts();
    let offline = test_message(
        &pair,
        "offline-first",
        &offline_conversation,
        &offline_did,
        "offline",
        now,
    );
    context
        .message_store
        .persist_message(&offline, false, now)
        .unwrap();
    let snapshot = pair.store_a.snapshot().unwrap();
    let reachable_did = snapshot
        .contacts()
        .iter()
        .find(|contact| contact.conversation_id() == pair.conversation_id)
        .unwrap()
        .remote_profile_did()
        .to_string();
    let reachable = test_message(
        &pair,
        "reachable-second",
        &pair.conversation_id,
        &reachable_did,
        "reachable",
        now,
    );
    context
        .message_store
        .persist_message(&reachable, false, now)
        .unwrap();

    assert!(direct
        .retry_pending(&pair.profile_a.document().profile_did, now)
        .await
        .is_err());
    let records = context.message_store.records().unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        !records
            .iter()
            .find(|record| record.envelope_bytes == offline)
            .unwrap()
            .receipt_settled
    );
    assert!(
        records
            .iter()
            .find(|record| record.envelope_bytes == reachable)
            .unwrap()
            .receipt_settled
    );
    let remote = pair
        .service_b
        .direct_message_service()
        .context(&pair.profile_b.document().profile_did, now_ts())
        .unwrap()
        .message_store
        .records()
        .unwrap();
    assert_eq!(remote.iter().filter(|record| record.incoming).count(), 1);
    assert_eq!(records[0].envelope_bytes, offline);
}

#[tokio::test]
async fn persistence_rejects_invalid_messages_receipts_and_state_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let direct_a = pair.service_a.direct_message_service();
    let direct_b = pair.service_b.direct_message_service();
    let context_a = direct_a
        .context(&pair.profile_a.document().profile_did, now_ts())
        .unwrap();
    let context_b = direct_b
        .context(&pair.profile_b.document().profile_did, now_ts())
        .unwrap();
    let now = now_ts();
    let device_b = pair.store_a.snapshot().unwrap().contacts()[0]
        .remote_profile_did()
        .to_string();
    let valid = test_message(
        &pair,
        "matrix-request",
        &pair.conversation_id,
        &device_b,
        "valid",
        now,
    );
    context_a
        .message_store
        .persist_message(&valid, false, now)
        .unwrap();
    let receiver_before_substitution = protected_store_bytes(&context_b.message_store);
    let foreign_endpoint =
        crate::crypto::encode_signing_key_did(&elastos_runtime::signature::generate_keypair().0);
    assert!(direct_b.receive(&valid, &foreign_endpoint, now).is_err());
    assert_eq!(
        protected_store_bytes(&context_b.message_store),
        receiver_before_substitution
    );
    let exact_receipt = direct_b
        .receive(&valid, &fixture_source_endpoint(&valid), now)
        .unwrap();
    let before_a = protected_store_bytes(&context_a.message_store);
    let before_b = protected_store_bytes(&context_b.message_store);

    let other_key =
        SigningKey::from_bytes(&elastos_runtime::signature::generate_keypair().0.to_bytes());
    let other_did = crate::crypto::encode_signing_key_did(&other_key);
    let wrong_network = resign_message(&valid, &pair.key_a, |message| {
        message.network_id = "other-network".to_string();
    });
    let wrong_sender = resign_message(&valid, &other_key, |_| {});
    let wrong_kind = resign_message(&valid, &pair.key_a, |message| {
        message.recipient.kind = CollaborationRecipientKind::Conversation;
    });
    let unknown_payload = resign_message(&valid, &pair.key_a, |message| {
        message.payload = serde_json::json!({
            "request_id":"matrix-request",
            "text":"valid",
            "unknown":true
        });
    });
    let empty_text = resign_message(&valid, &pair.key_a, |message| {
        message.payload = serde_json::to_value(DirectMessagePayload {
            request_id: "matrix-empty".to_string(),
            text: String::new(),
        })
        .unwrap();
    });
    let oversized_text = resign_message(&valid, &pair.key_a, |message| {
        message.payload = serde_json::to_value(DirectMessagePayload {
            request_id: "matrix-oversized".to_string(),
            text: "x".repeat(MAX_DIRECT_MESSAGE_TEXT_BYTES + 1),
        })
        .unwrap();
    });
    let invalid_request_id = resign_message(&valid, &pair.key_a, |message| {
        message.payload = serde_json::to_value(DirectMessagePayload {
            request_id: "not allowed".to_string(),
            text: "valid".to_string(),
        })
        .unwrap();
    });
    let mut bad_signature: SignedCollaborationMessage = serde_json::from_slice(&valid).unwrap();
    bad_signature.signature.replace_range(0..1, "A");
    let bad_signature = canonical_signed_collaboration_message_bytes(&bad_signature).unwrap();

    for (label, bytes) in [
        ("wrong network", wrong_network),
        ("wrong sender", wrong_sender),
        ("recipient kind", wrong_kind),
        ("unknown payload", unknown_payload),
        ("empty text", empty_text),
        ("oversized text", oversized_text),
        ("invalid request id", invalid_request_id),
        ("signature", bad_signature),
    ] {
        assert!(
            context_a
                .message_store
                .persist_message(&bytes, false, now)
                .is_err(),
            "{label} must fail"
        );
        assert_eq!(protected_store_bytes(&context_a.message_store), before_a);
    }

    assert!(context_a
        .message_store
        .persist_message(&valid, true, now)
        .is_err());
    assert_eq!(protected_store_bytes(&context_a.message_store), before_a);
    let wrong_recipient = resign_message(&valid, &pair.key_a, |message| {
        message.recipient.id = other_did.clone();
    });
    assert!(context_b
        .message_store
        .persist_message(&wrong_recipient, true, now)
        .is_err());
    assert_eq!(protected_store_bytes(&context_b.message_store), before_b);
    let wrong_conversation = resign_message(&valid, &pair.key_a, |message| {
        message.conversation_id = "wrong-direct-conversation".to_string();
    });
    assert!(direct_b
        .receive(
            &wrong_conversation,
            &fixture_source_endpoint(&wrong_conversation),
            now,
        )
        .is_err());
    assert_eq!(protected_store_bytes(&context_b.message_store), before_b);

    let raw_valid: SignedCollaborationMessage = serde_json::from_slice(&valid).unwrap();
    for field in ["message_id", "nonce"] {
        let duplicate = test_message(
            &pair,
            &format!("duplicate-{field}"),
            &pair.conversation_id,
            &device_b,
            "duplicate",
            now,
        );
        let duplicate = resign_message(&duplicate, &pair.key_a, |message| {
            if field == "message_id" {
                message.message_id = raw_valid.payload.message_id.clone();
            } else {
                message.nonce = raw_valid.payload.nonce.clone();
            }
        });
        assert!(context_a
            .message_store
            .persist_message(&duplicate, false, now)
            .is_err());
        assert_eq!(protected_store_bytes(&context_a.message_store), before_a);
    }

    let message_hash = collaboration_message_envelope_sha256(&valid);
    let mut altered_receipt = exact_receipt.clone();
    let final_byte = altered_receipt.len() - 1;
    altered_receipt[final_byte] ^= 1;
    assert!(context_a
        .message_store
        .persist_receipt(&message_hash, &altered_receipt, now)
        .is_err());
    assert!(context_a
        .message_store
        .persist_receipt("sha256:missing", &exact_receipt, now)
        .is_err());
    let other_message = test_message(
        &pair,
        "other-receipt-request",
        &pair.conversation_id,
        &device_b,
        "other",
        now,
    );
    let other_receipt = direct_b
        .receive(
            &other_message,
            &fixture_source_endpoint(&other_message),
            now,
        )
        .unwrap();
    assert!(context_a
        .message_store
        .persist_receipt(&message_hash, &other_receipt, now)
        .is_err());
    assert_eq!(protected_store_bytes(&context_a.message_store), before_a);

    let canonical_state =
        serde_json::to_vec(&context_a.message_store.load().unwrap().unwrap()).unwrap();
    let uri = context_a.message_store.object_uri();
    let path = context_a.message_store.object_path().unwrap();
    let mut noncanonical = canonical_state.clone();
    noncanonical.push(b'\n');
    write_protected_principal_root_object(
        &context_a.message_store.data_root,
        &context_a.message_store.principal_id,
        &context_a.message_store.localhost_root,
        &uri,
        &path,
        &noncanonical,
    )
    .unwrap();
    assert!(context_a.message_store.records().is_err());
    write_protected_principal_root_object(
        &context_a.message_store.data_root,
        &context_a.message_store.principal_id,
        &context_a.message_store.localhost_root,
        &uri,
        &path,
        &vec![b'x'; MAX_DIRECT_MESSAGE_STATE_BYTES + 1],
    )
    .unwrap();
    assert!(context_a.message_store.records().is_err());
    write_protected_principal_root_object(
        &context_a.message_store.data_root,
        &context_a.message_store.principal_id,
        &context_a.message_store.localhost_root,
        &uri,
        &path,
        &canonical_state,
    )
    .unwrap();
    assert_eq!(context_a.message_store.records().unwrap().len(), 1);
}

#[tokio::test]
async fn accepted_contacts_exchange_direct_messages_without_bootstrap_peers_or_seed_online() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    assert!(pair
        .service_a
        .network_profile()
        .profile()
        .bootstrap_peers
        .is_empty());
    assert!(pair
        .service_b
        .network_profile()
        .profile()
        .bootstrap_peers
        .is_empty());
    assert!(!pair.store_a.discovery_enabled().unwrap());
    assert!(!pair.store_b.discovery_enabled().unwrap());
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let direct_a = pair.service_a.direct_message_service();
    let direct_b = pair.service_b.direct_message_service();
    let outcome_a = direct_a
        .send_text(
            &pair.profile_a.document().profile_did,
            "direct-request-a",
            &pair.conversation_id,
            "hello b",
            now_ts(),
        )
        .await
        .unwrap();
    if outcome_a == DirectDeliveryStatus::Pending {
        let context = direct_a
            .context(&pair.profile_a.document().profile_did, now_ts())
            .unwrap();
        let record = context
            .message_store
            .records()
            .unwrap()
            .into_iter()
            .find(|record| !record.incoming)
            .unwrap();
        pair.service_b
            .direct_message_service()
            .receive(
                &record.envelope_bytes,
                &fixture_source_endpoint(&record.envelope_bytes),
                now_ts(),
            )
            .expect("remote direct receiver must accept the exact contact message");
        let remote_did = pair.store_a.snapshot().unwrap().contacts()[0]
            .remote_presence_device_did()
            .to_string();
        let error = pair
            .registry_a
            .invoke_provider(ProviderInvocation {
                source: DIRECT_MESSAGE_PROVIDER_SCHEME.to_string(),
                target: DIRECT_MESSAGE_PROVIDER_SCHEME.to_string(),
                op: DIRECT_MESSAGE_PROVIDER_OP.to_string(),
                request: serde_json::to_value(DirectDeliveryRequest {
                    op: DIRECT_MESSAGE_PROVIDER_OP.to_string(),
                    message: encode(&record.envelope_bytes),
                })
                .unwrap(),
                transfer: ProviderTransfer::Json,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                    peer_did: remote_did,
                    timeout_ms: Some(DIRECT_PROVIDER_TIMEOUT_MS),
                }),
            })
            .await
            .unwrap_err();
        panic!("direct peer delivery failed: {error}");
    }
    assert_eq!(outcome_a, DirectDeliveryStatus::ReceiptSettled);
    assert_eq!(
        direct_b
            .send_text(
                &pair.profile_b.document().profile_did,
                "direct-request-b",
                &pair.conversation_id,
                "hello a",
                now_ts(),
            )
            .await
            .unwrap(),
        DirectDeliveryStatus::ReceiptSettled,
    );
    let context_a = direct_a
        .context(&pair.profile_a.document().profile_did, now_ts())
        .unwrap();
    let context_b = direct_b
        .context(&pair.profile_b.document().profile_did, now_ts())
        .unwrap();
    let records_a = context_a.message_store.records().unwrap();
    let records_b = context_b.message_store.records().unwrap();
    assert_eq!(records_a.iter().filter(|record| record.incoming).count(), 1);
    assert_eq!(
        records_a.iter().filter(|record| !record.incoming).count(),
        1
    );
    assert_eq!(records_b.iter().filter(|record| record.incoming).count(), 1);
    assert_eq!(
        records_b.iter().filter(|record| !record.incoming).count(),
        1
    );
    assert!(records_a
        .iter()
        .filter(|record| !record.incoming)
        .all(|record| record.receipt_settled));
    assert!(records_b
        .iter()
        .filter(|record| !record.incoming)
        .all(|record| record.receipt_settled));
    let outgoing_a = records_a.iter().find(|record| !record.incoming).unwrap();
    let receipt_one = direct_b
        .receive(
            &outgoing_a.envelope_bytes,
            &fixture_source_endpoint(&outgoing_a.envelope_bytes),
            now_ts(),
        )
        .unwrap();
    let receipt_two = direct_b
        .receive(
            &outgoing_a.envelope_bytes,
            &fixture_source_endpoint(&outgoing_a.envelope_bytes),
            now_ts(),
        )
        .unwrap();
    assert_eq!(receipt_one, receipt_two);
    assert_eq!(
        direct_a
            .send_text(
                &pair.profile_a.document().profile_did,
                "direct-request-a",
                &pair.conversation_id,
                "hello b",
                now_ts(),
            )
            .await
            .unwrap(),
        DirectDeliveryStatus::ReceiptSettled,
    );
    assert!(direct_a
        .send_text(
            &pair.profile_a.document().profile_did,
            "direct-request-a",
            &pair.conversation_id,
            "changed",
            now_ts(),
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("request_id conflicts"));
    assert_eq!(
        pair.store_a.snapshot().unwrap().contacts()[0].conversation_id(),
        pair.conversation_id
    );
    assert_eq!(
        pair.store_b.snapshot().unwrap().contacts()[0].conversation_id(),
        pair.conversation_id
    );
    assert_eq!(
        pair.registry_a
            .schemes()
            .await
            .iter()
            .filter(|scheme| *scheme == "collaboration")
            .count(),
        1
    );
    let schemes_b = pair.registry_b.schemes().await;
    assert!(schemes_b.iter().any(|scheme| scheme == "collaboration"));
    assert!(schemes_b
        .iter()
        .any(|scheme| scheme == DIRECT_MESSAGE_PROVIDER_SCHEME));
    assert_eq!(
        pair.registry_a
            .schemes()
            .await
            .iter()
            .filter(|scheme| *scheme == DIRECT_MESSAGE_PROVIDER_SCHEME)
            .count(),
        1
    );
}

#[tokio::test]
async fn direct_delivery_leaves_on_the_peer_did_route_without_bootstrap_transport() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    assert!(pair
        .service_a
        .network_profile()
        .profile()
        .bootstrap_peers
        .is_empty());
    assert!(pair
        .service_b
        .network_profile()
        .profile()
        .bootstrap_peers
        .is_empty());

    let invoker = std::sync::Arc::new(RecordingDirectCarrierInvoker {
        source_endpoint_did: crate::crypto::encode_signing_key_did(&pair.key_a),
        remote: pair.service_b.direct_message_service(),
        calls: tokio::sync::Mutex::new(Vec::new()),
    });
    pair.registry_a.set_carrier_invoker(invoker.clone()).await;

    let outcome = pair
        .service_a
        .direct_message_service()
        .send_text(
            &pair.profile_a.document().profile_did,
            "exact-peer-route",
            &pair.conversation_id,
            "hello over peer did",
            now_ts(),
        )
        .await
        .unwrap();
    assert_eq!(outcome, DirectDeliveryStatus::ReceiptSettled);

    let calls = invoker.calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].source, DIRECT_MESSAGE_PROVIDER_SCHEME);
    assert_eq!(calls[0].target, DIRECT_MESSAGE_PROVIDER_SCHEME);
    assert_eq!(calls[0].op, DIRECT_MESSAGE_PROVIDER_OP);
    assert_eq!(
        calls[0].peer_did,
        pair.store_a.snapshot().unwrap().contacts()[0].remote_presence_device_did()
    );
    assert_eq!(
        calls[0].request["_runtime_invocation"]["transport"],
        "carrier-provider-plane"
    );
    assert_eq!(
        calls[0].request["_runtime_invocation"]["carrier"],
        serde_json::Value::Null
    );
    let rendered = calls[0].request.to_string();
    assert!(!rendered.contains("connect_ticket"));
    assert!(!rendered.contains("bootstrap"));
    let envelope = decode(
        calls[0].request["message"].as_str().unwrap(),
        "direct message",
    )
    .unwrap();
    let verified = verify_direct_message(
        &envelope,
        &pair.service_a.network_profile(),
        &pair.conversation_id,
        &pair.profile_a,
        &pair.profile_b,
        now_ts(),
    )
    .unwrap();
    let payload: DirectMessagePayload =
        serde_json::from_value(verified.envelope().payload.payload.clone()).unwrap();
    assert_eq!(payload.request_id, "exact-peer-route");
    assert_eq!(payload.text, "hello over peer did");
}

#[tokio::test]
async fn direct_provider_rejects_a_self_route_rewritten_to_runtime_local() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let now = now_ts();
    let message = test_message(
        &pair,
        "self-local-rewrite",
        &pair.conversation_id,
        &pair.profile_b.document().profile_did,
        "must retain Carrier admission",
        now,
    );
    let before = pair
        .service_b
        .direct_message_service()
        .records_for_test(&pair.profile_b.document().profile_did, now)
        .unwrap();

    let error = pair
        .registry_b
        .invoke_provider(ProviderInvocation {
            source: DIRECT_MESSAGE_PROVIDER_SCHEME.to_string(),
            target: DIRECT_MESSAGE_PROVIDER_SCHEME.to_string(),
            op: DIRECT_MESSAGE_PROVIDER_OP.to_string(),
            request: serde_json::to_value(DirectDeliveryRequest {
                op: DIRECT_MESSAGE_PROVIDER_OP.to_string(),
                message: encode(&message),
            })
            .unwrap(),
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("invalid direct message provider invocation"));
    assert_eq!(
        pair.service_b
            .direct_message_service()
            .records_for_test(&pair.profile_b.document().profile_did, now)
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn same_runtime_profiles_settle_through_authenticated_carrier_loopback_without_a_dial() {
    let temp = tempfile::tempdir().unwrap();
    let mut pair =
        crate::collaboration_discovery_runtime::tests::same_runtime_profile_pair(temp.path()).await;
    let endpoint_did = crate::crypto::encode_signing_key_did(&pair.endpoint_key);
    pair.node
        .take()
        .expect("fixture Carrier node")
        .shutdown()
        .await;

    let direct = pair.service.direct_message_service();
    let now = now_ts();
    assert_eq!(
        direct
            .send_text(
                &pair.profile_a.document().profile_did,
                "same-runtime-direct",
                &pair.conversation_id,
                "one Runtime, exact Carrier admission",
                now,
            )
            .await
            .unwrap(),
        DirectDeliveryStatus::ReceiptSettled,
    );

    let sender = direct
        .context(&pair.profile_a.document().profile_did, now)
        .unwrap();
    let recipient = direct
        .context(&pair.profile_b.document().profile_did, now)
        .unwrap();
    let sender_records = sender.message_store.records().unwrap();
    let recipient_records = recipient.message_store.records().unwrap();
    assert_eq!(sender_records.len(), 1);
    assert!(!sender_records[0].incoming);
    assert!(sender_records[0].receipt_settled);
    assert_eq!(recipient_records.len(), 1);
    assert!(recipient_records[0].incoming);
    assert_eq!(
        recipient_records[0].envelope_bytes,
        sender_records[0].envelope_bytes
    );

    let message_hash = collaboration_message_envelope_sha256(&sender_records[0].envelope_bytes);
    let sender_receipt = sender
        .message_store
        .receipt(&message_hash)
        .unwrap()
        .expect("sender persisted terminal receipt");
    let receiver_receipt = direct
        .receive(&sender_records[0].envelope_bytes, &endpoint_did, now + 1)
        .unwrap();
    assert_eq!(sender_receipt, receiver_receipt);
}

#[tokio::test]
async fn same_runtime_carrier_loopback_rejects_wrong_authority_and_recipient_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let mut pair =
        crate::collaboration_discovery_runtime::tests::same_runtime_profile_pair(temp.path()).await;
    let endpoint_did = crate::crypto::encode_signing_key_did(&pair.endpoint_key);
    pair.node
        .take()
        .expect("fixture Carrier node")
        .shutdown()
        .await;
    let now = now_ts();
    let valid = prepare_direct_message(
        &pair.endpoint_key,
        &pair.service.network_profile(),
        &pair.profile_a,
        DirectMessageIntent {
            request_id: "same-runtime-negative",
            conversation_id: &pair.conversation_id,
            recipient_profile_did: &pair.profile_b.document().profile_did,
            text: "must stay authority bound",
        },
        now,
    )
    .unwrap();
    let invocation = |target: &str, op: &str, message: &[u8]| ProviderInvocation {
        source: DIRECT_MESSAGE_PROVIDER_SCHEME.to_string(),
        target: target.to_string(),
        op: op.to_string(),
        request: serde_json::json!({
            "op": op,
            "message": encode(message),
        }),
        transfer: ProviderTransfer::Json,
        range: None,
        progress: None,
        transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
            peer_did: endpoint_did.clone(),
            timeout_ms: Some(DIRECT_PROVIDER_TIMEOUT_MS),
        }),
    };
    let before = pair
        .service
        .direct_message_service()
        .records_for_test(&pair.profile_b.document().profile_did, now)
        .unwrap();

    assert!(pair
        .registry
        .invoke_provider(invocation(
            crate::collaboration_profile_updates::PROFILE_UPDATE_PROVIDER_SCHEME,
            DIRECT_MESSAGE_PROVIDER_OP,
            &valid,
        ))
        .await
        .is_err());
    assert!(pair
        .registry
        .invoke_provider(invocation(DIRECT_MESSAGE_PROVIDER_SCHEME, "query", &valid))
        .await
        .is_err());

    let mut forged = invocation(
        DIRECT_MESSAGE_PROVIDER_SCHEME,
        DIRECT_MESSAGE_PROVIDER_OP,
        &valid,
    );
    forged.request["_runtime_invocation"] = serde_json::json!({
        "transport": "carrier-provider-plane",
        "carrier": { "source_endpoint_did": endpoint_did },
    });
    assert!(pair.registry.invoke_provider(forged).await.is_err());

    let wrong_registered_recipient = resign_message(&valid, &pair.endpoint_key, |message| {
        message.recipient.id = pair.profile_a.document().profile_did.clone();
    });
    assert!(pair
        .registry
        .invoke_provider(invocation(
            DIRECT_MESSAGE_PROVIDER_SCHEME,
            DIRECT_MESSAGE_PROVIDER_OP,
            &wrong_registered_recipient,
        ))
        .await
        .is_err());
    let unregistered_profile =
        crate::crypto::encode_signing_key_did(&elastos_runtime::signature::generate_keypair().0);
    let unregistered_recipient = resign_message(&valid, &pair.endpoint_key, |message| {
        message.recipient.id = unregistered_profile;
    });
    assert!(pair
        .registry
        .invoke_provider(invocation(
            DIRECT_MESSAGE_PROVIDER_SCHEME,
            DIRECT_MESSAGE_PROVIDER_OP,
            &unregistered_recipient,
        ))
        .await
        .is_err());
    assert_eq!(
        pair.service
            .direct_message_service()
            .records_for_test(&pair.profile_b.document().profile_did, now)
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn non_recipient_runtime_cannot_admit_another_profiles_direct_message() {
    let temp = tempfile::tempdir().unwrap();
    let pair = crate::collaboration_discovery_runtime::tests::direct_peer_pair(temp.path()).await;
    let now = now_ts();
    let message = test_message(
        &pair,
        "for-b-only",
        &pair.conversation_id,
        &pair.profile_b.document().profile_did,
        "only bob may receive this",
        now,
    );
    assert!(pair
        .service_a
        .direct_message_service()
        .receive(&message, &fixture_source_endpoint(&message), now)
        .is_err());
    assert!(pair
        .service_a
        .direct_message_service()
        .context(&pair.profile_a.document().profile_did, now)
        .unwrap()
        .message_store
        .records()
        .unwrap()
        .is_empty());
}

#[test]
fn direct_provider_runtime_metadata_is_exact() {
    let source_endpoint_did =
        crate::crypto::encode_signing_key_did(&elastos_runtime::signature::generate_keypair().0);
    let valid = serde_json::json!({
        "schema":"elastos.provider.invocation/v1",
        "source":DIRECT_MESSAGE_PROVIDER_SCHEME,
        "target":DIRECT_MESSAGE_PROVIDER_SCHEME,
        "op":DIRECT_MESSAGE_PROVIDER_OP,
        "capability":format!("provider:{0}->{0}:{1}", DIRECT_MESSAGE_PROVIDER_SCHEME, DIRECT_MESSAGE_PROVIDER_OP),
        "transport":"carrier-provider-plane",
        "transfer":"json",
        "carrier":{"source_endpoint_did":source_endpoint_did}
    });
    assert_eq!(
        validate_direct_runtime_invocation(Some(&valid)).unwrap(),
        source_endpoint_did
    );
    for (field, replacement) in [
        ("source", serde_json::json!("other")),
        ("target", serde_json::json!("collaboration")),
        ("op", serde_json::json!("query")),
        ("transfer", serde_json::json!("bytes")),
        (
            "transport",
            serde_json::json!("runtime-local-provider-plane"),
        ),
        ("carrier", serde_json::Value::Null),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = replacement;
        assert!(
            validate_direct_runtime_invocation(Some(&invalid)).is_err(),
            "{field}"
        );
    }
    let mut caller_asserted = valid;
    caller_asserted["carrier"]["peer_did"] = serde_json::json!(source_endpoint_did);
    assert!(validate_direct_runtime_invocation(Some(&caller_asserted)).is_err());
}

#[test]
fn direct_message_signer_and_carrier_endpoint_are_independent_profile_roles() {
    let profile_key = SigningKey::from_bytes(&[71u8; 32]);
    let endpoint_key = SigningKey::from_bytes(&[72u8; 32]);
    let message_key = SigningKey::from_bytes(&[73u8; 32]);
    let recipient_profile_key = SigningKey::from_bytes(&[74u8; 32]);
    let recipient_endpoint_key = SigningKey::from_bytes(&[75u8; 32]);
    let endpoint_did = crate::crypto::encode_signing_key_did(&endpoint_key);
    let message_signer_did = crate::crypto::encode_signing_key_did(&message_key);
    let sender_profile =
        crate::collaboration_profile_authority::signed_profile_document_with_authority_for_test(
            &profile_key,
            "Alice",
            None,
            1,
            None,
            1_800_000_000,
            crate::collaboration_profile_authority::ProfileAuthorityForTest {
                endpoint_dids: vec![endpoint_did.clone()],
                signer_dids: vec![message_signer_did.clone()],
            },
        )
        .unwrap();
    let recipient_profile =
        crate::collaboration_profile_authority::signed_profile_document_for_test(
            &recipient_profile_key,
            "Bob",
            None,
            1,
            None,
            1_800_000_000,
            vec![crate::crypto::encode_signing_key_did(
                &recipient_endpoint_key,
            )],
        )
        .unwrap();
    let trusted = SigningKey::from_bytes(&[76u8; 32]);
    let network = crate::collaboration_discovery_runtime::tests::signed_profile(
        "direct-role-separation-test",
        &trusted,
        vec![],
    );
    let now = 1_800_000_100;
    let envelope = prepare_direct_message(
        &message_key,
        &network,
        &sender_profile,
        DirectMessageIntent {
            request_id: "separate-role-request",
            conversation_id:
                "direct:sha256:aa11bb22cc33dd44ee55ff6677889900aabbccddeeff00112233445566778899",
            recipient_profile_did: &recipient_profile.document().profile_did,
            text: "endpoint and signer are separate",
        },
        now,
    )
    .unwrap();

    let verified = verify_direct_message(
        &envelope,
        &network,
        "direct:sha256:aa11bb22cc33dd44ee55ff6677889900aabbccddeeff00112233445566778899",
        &sender_profile,
        &recipient_profile,
        now,
    )
    .unwrap();
    assert_eq!(verified.envelope().signer_did, message_signer_did);
    assert_ne!(verified.envelope().signer_did, endpoint_did);
    assert_eq!(sender_profile.sole_endpoint_did().unwrap(), endpoint_did);
}

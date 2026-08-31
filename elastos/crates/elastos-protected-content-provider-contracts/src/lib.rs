#![forbid(unsafe_code)]

//! Private typed Runtime-to-provider transport contracts for protected content.
//!
//! This crate does not define authority. It transports exact canonical
//! protected-content v1 authority objects between Runtime and later rights,
//! custody, and decrypt providers.

mod custody;
mod decrypt;
mod media;
mod protect;
mod rights;
#[cfg(test)]
mod test_support;
mod wire;

pub use custody::{
    CustodyProviderRequestOpV1, CustodyProviderRequestV1, CustodyProviderRequestValidationErrorV1,
    CustodyProviderResponseStatusV1, CustodyProviderResponseV1, ValidatedCustodyProviderRequestV1,
    ValidatedCustodyProvisionNodeShareRequestV1, ValidatedCustodyReleaseContributionRequestV1,
    CUSTODY_PROVIDER_REQUEST_SCHEMA_V1, CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1,
};
pub use decrypt::{
    DecryptProviderRequestOpV1, DecryptProviderRequestV1, DecryptProviderResponseStatusV1,
    DecryptProviderResponseV1, ValidatedDecryptProviderRequestV1, ViewerMediaPartSelectorV1,
    DECRYPT_PROVIDER_REQUEST_SCHEMA_V1, DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
    MAX_VIEWER_MEDIA_PART_BYTES_V1,
};
pub use media::{
    CencFmp4MediaIdentityV1, CencFmp4SegmentIdentityV1, ValidatedCencFmp4MediaLayoutV1,
    ValidatedCencFmp4MediaSessionLayoutV1, ValidatedCencFmp4SampleLayoutV1,
    ValidatedCencFmp4SegmentLayoutV1, ValidatedCencFmp4SubsampleV1,
    ValidatedClearFmp4MediaSessionLayoutV1, ValidatedClearFmp4SampleLayoutV1,
    ValidatedClearFmp4SegmentLayoutV1, CENC_FMP4_MEDIA_SUITE_ID_V1,
    MAX_CENC_FMP4_MEDIA_IDENTITY_BYTES_V1,
};
pub use protect::{
    ProtectProviderRequestOpV1, ProtectProviderRequestV1, ProtectProviderResponseStatusV1,
    ProtectProviderResponseV1, ProtectionSessionNodeV1, MAX_PROTECT_MEDIA_PART_BYTES_V1,
    MAX_PROTECT_MEDIA_SEGMENTS_V1, PROTECT_PROVIDER_REQUEST_SCHEMA_V1,
    PROTECT_PROVIDER_RESPONSE_SCHEMA_V1,
};
pub use rights::{
    RightsProviderRequestOpV1, RightsProviderRequestV1, RightsProviderResponseStatusV1,
    RightsProviderResponseV1, ValidatedRightsProviderRequestV1, RIGHTS_PROVIDER_REQUEST_SCHEMA_V1,
    RIGHTS_PROVIDER_RESPONSE_SCHEMA_V1,
};
pub use wire::{
    OpaqueHandleV1, ProviderFailureCodeV1, MAX_CUSTODY_ENVELOPE_BYTES_V1,
    MAX_CUSTODY_NODE_PROVISIONING_RECORD_BYTES_V1, MAX_PROVIDER_BINDING_BYTES_V1,
    MAX_PROVIDER_FRAME_BYTES_V1, MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
    MAX_RECIPIENT_IDENTITY_BYTES_V1, MAX_SIGNED_NODE_CONTRIBUTION_BYTES_V1,
    MAX_SIGNED_NODE_RIGHTS_DECISION_BYTES_V1, MAX_SIGNED_RUNTIME_CUSTODY_PROVISIONING_BYTES_V1,
    MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1, MAX_SIGNED_TERMINAL_RECEIPT_BYTES_V1,
};

#[cfg(test)]
mod tests {
    use super::{
        CustodyProviderRequestV1, CustodyProviderResponseV1, DecryptProviderRequestV1,
        DecryptProviderResponseV1, ProviderFailureCodeV1, RightsProviderRequestV1,
        RightsProviderResponseV1, ValidatedCustodyProviderRequestV1,
        ValidatedDecryptProviderRequestV1, ValidatedRightsProviderRequestV1,
        MAX_PROVIDER_FRAME_BYTES_V1, MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
    };
    use crate::test_support::{
        custody_envelope_for_media, make_signed_node_contribution,
        make_signed_node_rights_decision, make_signed_runtime_release_operation,
        make_signed_runtime_release_operation_for_envelope_and_seed, make_signed_terminal_receipt,
        media_components, media_identity, recipient_identity, recipient_public_key,
        runtime_operation_issuer_for_seed, NOW,
    };

    fn handle(seed: u8) -> [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
        let mut bytes = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        bytes[0] = seed.max(1);
        bytes[31] = seed ^ 0x5a;
        bytes
    }

    fn request_decoders() -> [fn(&[u8]) -> bool; 3] {
        [
            |bytes| {
                ValidatedRightsProviderRequestV1::decode_and_validate_at(bytes, NOW + 10).is_ok()
            },
            |bytes| {
                ValidatedCustodyProviderRequestV1::decode_and_validate_at(
                    bytes,
                    runtime_operation_issuer_for_seed(0x42),
                    crate::test_support::node_public_key(1),
                    NOW + 10,
                )
                .is_ok()
            },
            |bytes| {
                ValidatedDecryptProviderRequestV1::decode_and_validate_at(
                    bytes,
                    runtime_operation_issuer_for_seed(0x42),
                    NOW + 10,
                )
                .is_ok()
            },
        ]
    }

    fn response_decoders() -> [fn(&[u8]) -> bool; 3] {
        [
            |bytes| RightsProviderResponseV1::from_json_slice(bytes).is_ok(),
            |bytes| CustodyProviderResponseV1::from_json_slice(bytes).is_ok(),
            |bytes| DecryptProviderResponseV1::from_json_slice(bytes).is_ok(),
        ]
    }

    #[test]
    fn provider_protocols_reject_wrong_schema_and_request_response_confusion() {
        let media_identity = media_identity(0x11);
        let (init_segment, _, _, _) = media_components(0x11);
        let custody_envelope = custody_envelope_for_media(0x11);
        let operation =
            make_signed_runtime_release_operation_for_envelope_and_seed(0x42, &custody_envelope);
        let decision = make_signed_node_rights_decision(
            &operation,
            1,
            elastos_protected_content_contracts::RightsDecisionV1::Allowed,
        );
        let contribution = make_signed_node_contribution(&operation, 1);
        let contribution_two = make_signed_node_contribution(&operation, 2);
        let terminal = make_signed_terminal_receipt(
            &operation,
            &[contribution.clone(), contribution_two],
            0x61,
        );

        let rights_request = RightsProviderRequestV1::new_evaluate(
            decision.statement().node_public_key(),
            &operation,
        )
        .unwrap();
        let rights_response = RightsProviderResponseV1::new_decision(&decision).unwrap();
        let custody_request =
            CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap();
        let custody_response = CustodyProviderResponseV1::new_contribution(&contribution).unwrap();
        let decrypt_request = DecryptProviderRequestV1::new_open_viewer_session(
            handle(0x21),
            &operation,
            terminal.statement().issuer(),
            custody_envelope.manifest().content_key_commitment(),
            &media_identity,
            &init_segment,
            &[
                contribution.clone(),
                make_signed_node_contribution(&operation, 2),
            ],
            &terminal,
        )
        .unwrap();
        let decrypt_response = DecryptProviderResponseV1::new_prepared_recipient(
            operation.statement().audit_request_id(),
            handle(0x31),
            recipient_public_key(0x30),
            &recipient_identity(0x30),
        )
        .unwrap();

        let request_cases = [
            rights_request.to_json_vec().unwrap(),
            custody_request.to_json_vec().unwrap(),
            decrypt_request.to_json_vec().unwrap(),
        ];
        let response_cases = [
            rights_response.to_json_vec().unwrap(),
            custody_response.to_json_vec().unwrap(),
            decrypt_response.to_json_vec().unwrap(),
        ];

        for (json_bytes, decode_response) in request_cases.iter().zip(response_decoders()) {
            assert!(!decode_response(json_bytes));
        }
        for (json_bytes, decode_request) in response_cases.iter().zip(request_decoders()) {
            assert!(!decode_request(json_bytes));
        }

        for (json_bytes, decode_request, decode_response) in [
            (
                rights_request.to_json_vec().unwrap(),
                request_decoders()[0],
                response_decoders()[0],
            ),
            (
                custody_request.to_json_vec().unwrap(),
                request_decoders()[1],
                response_decoders()[1],
            ),
            (
                decrypt_request.to_json_vec().unwrap(),
                request_decoders()[2],
                response_decoders()[2],
            ),
            (
                rights_response.to_json_vec().unwrap(),
                request_decoders()[0],
                response_decoders()[0],
            ),
            (
                custody_response.to_json_vec().unwrap(),
                request_decoders()[1],
                response_decoders()[1],
            ),
            (
                decrypt_response.to_json_vec().unwrap(),
                request_decoders()[2],
                response_decoders()[2],
            ),
        ] {
            let mut value: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();
            value["schema"] = serde_json::Value::String("wrong.schema/v1".to_string());
            let wrong_schema = serde_json::to_vec(&value).unwrap();
            assert!(!decode_request(&wrong_schema));
            assert!(!decode_response(&wrong_schema));
        }
    }

    #[test]
    fn provider_frame_rejects_limit_plus_one_for_all_protocols() {
        let oversized = vec![b' '; MAX_PROVIDER_FRAME_BYTES_V1 + 1];
        for decode in request_decoders().into_iter().chain(response_decoders()) {
            assert!(!decode(b""));
            assert!(!decode(&oversized));
        }
    }

    #[test]
    fn decrypt_terminal_lifecycle_responses_round_trip_with_exact_handles() {
        let audit_id = make_signed_runtime_release_operation()
            .statement()
            .audit_request_id();
        let prepared = handle(0x21);
        let viewer = handle(0x31);
        let responses = [
            DecryptProviderResponseV1::new_cancelled_prepared_recipient(audit_id, prepared)
                .unwrap(),
            DecryptProviderResponseV1::new_prepared_recipient_already_absent(audit_id, prepared)
                .unwrap(),
            DecryptProviderResponseV1::new_closed_viewer_session(audit_id, viewer).unwrap(),
            DecryptProviderResponseV1::new_viewer_session_already_absent(audit_id, viewer).unwrap(),
            DecryptProviderResponseV1::new_failure(audit_id, ProviderFailureCodeV1::HandleAbsent)
                .unwrap(),
        ];

        for response in responses {
            let decoded =
                DecryptProviderResponseV1::from_json_slice(&response.to_json_vec().unwrap())
                    .unwrap();
            assert_eq!(decoded, response);
        }
    }

    #[test]
    fn serialized_messages_do_not_contain_forbidden_transport_or_secret_fields() {
        let media_identity = media_identity(0x11);
        let (init_segment, _, _, _) = media_components(0x11);
        let custody_envelope = custody_envelope_for_media(0x11);
        let operation =
            make_signed_runtime_release_operation_for_envelope_and_seed(0x42, &custody_envelope);
        let decision = make_signed_node_rights_decision(
            &operation,
            1,
            elastos_protected_content_contracts::RightsDecisionV1::Allowed,
        );
        let contribution = make_signed_node_contribution(&operation, 1);
        let contribution_two = make_signed_node_contribution(&operation, 2);
        let terminal = make_signed_terminal_receipt(
            &operation,
            &[contribution.clone(), contribution_two],
            0x61,
        );
        let messages = [
            RightsProviderRequestV1::new_evaluate(
                decision.statement().node_public_key(),
                &operation,
            )
            .unwrap()
            .to_json_vec()
            .unwrap(),
            RightsProviderResponseV1::new_decision(&decision)
                .unwrap()
                .to_json_vec()
                .unwrap(),
            CustodyProviderRequestV1::new_release_contribution(&operation, &decision)
                .unwrap()
                .to_json_vec()
                .unwrap(),
            CustodyProviderResponseV1::new_contribution(&contribution)
                .unwrap()
                .to_json_vec()
                .unwrap(),
            DecryptProviderRequestV1::new_open_viewer_session(
                handle(0x21),
                &operation,
                terminal.statement().issuer(),
                custody_envelope.manifest().content_key_commitment(),
                &media_identity,
                &init_segment,
                &[
                    contribution.clone(),
                    make_signed_node_contribution(&operation, 2),
                ],
                &terminal,
            )
            .unwrap()
            .to_json_vec()
            .unwrap(),
            DecryptProviderResponseV1::new_prepared_recipient(
                operation.statement().audit_request_id(),
                handle(0x31),
                recipient_public_key(0x30),
                &recipient_identity(0x30),
            )
            .unwrap()
            .to_json_vec()
            .unwrap(),
        ];

        for bytes in messages {
            let json = String::from_utf8(bytes).unwrap();
            for forbidden in [
                "raw_share",
                "raw_cek",
                "\"cek\"",
                "endpoint",
                "\"ip\"",
                "\"port\"",
                "credential",
                "rpc_url",
                "topology",
                "open_session",
                "\"render\"",
                "\"release\"",
            ] {
                assert!(
                    !json.contains(forbidden),
                    "unexpected {forbidden} in {json}"
                );
            }
        }
    }
}

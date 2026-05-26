use super::support::*;
use super::*;

#[test]
fn challenge_and_verify_evm_proof() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let signing_key = SigningKey::from_bytes((&[3u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);

    let challenge = provider.handle(Request::Challenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address: address.clone(),
        chain_id: 20,
        resources: vec!["elastos://wallet/account/link".into()],
    });
    let message = match challenge {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["schema"], AuthChallengeV1::SCHEMA);
            let resources = data["resources"].as_array().unwrap();
            assert!(resources
                .iter()
                .any(|resource| resource.as_str() == Some("elastos://wallet/account/link")));
            data["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected challenge, got {other:?}"),
    };
    let signature = sign_message(&signing_key, &message);
    let verified = provider.handle(Request::VerifyProof { message, signature });

    match verified {
        Response::Ok { data: Some(data) } => {
            assert_eq!(
                data["proof_binding_id"],
                format!("proof:wallet:eip155:20:{}", address.to_ascii_lowercase())
            );
            assert_eq!(data["chain_namespace"], "eip155:20");
            assert_eq!(data["address"], address.to_ascii_lowercase());
            assert_eq!(data["proof_type"], "siwe");
            assert!(data["message_hash"].as_str().unwrap().starts_with("0x"));
        }
        other => panic!("expected verified proof, got {other:?}"),
    }
}

#[test]
fn bip322_simple_p2wpkh_vector_verifies() {
    let address = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";
    let signature = "AkcwRAIgZRfIY3p7/DoVTty6YZbWS71bc5Vct9p9Fia83eRmw2QCICK/ENGfwLtptFluMGs2KsqoNSk89pO7F29zJLUx9a/sASECx/EgAxlkQpQ9hYjgGu6EBCPMVPwVIVJqO4XCsMvViHI=";

    let proof = verify_bip322_simple("bitcoin", address, "Hello World", signature).unwrap();

    assert_eq!(
        hex::encode(proof.message_hash),
        "f0eb03b1a75ac6d9847f55c624a99169b5dccba2a31f5b23bea77ba270de0a7a"
    );
    assert_eq!(
        proof.chain_namespace,
        "bip122:000000000019d6689c085ae165831e93"
    );
    assert_eq!(proof.address, address);
    assert_eq!(proof.proof_type, "bip322_simple");
    assert_eq!(proof.proof_strength, "standard");
}

#[test]
fn bip322_simple_p2wpkh_rejects_wrong_message() {
    let address = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";
    let signature = "AkcwRAIgZRfIY3p7/DoVTty6YZbWS71bc5Vct9p9Fia83eRmw2QCICK/ENGfwLtptFluMGs2KsqoNSk89pO7F29zJLUx9a/sASECx/EgAxlkQpQ9hYjgGu6EBCPMVPwVIVJqO4XCsMvViHI=";

    let err = verify_bip322_simple("bitcoin", address, "Wrong message", signature).unwrap_err();

    assert!(err.contains("invalid BIP-322 signature"));
}

#[test]
fn bip322_simple_p2tr_verifies() {
    let secret_key = bip322_test_taproot_secret_key();
    let address = bip322_test_taproot_address(&secret_key);
    assert!(address.starts_with("bc1p"));
    let message = "Hello Taproot";
    let signature = sign_bip322_simple_p2tr(&secret_key, &address, message);

    let proof = verify_bip322_simple("bitcoin", &address, message, &signature).unwrap();

    assert_eq!(
        proof.chain_namespace,
        "bip122:000000000019d6689c085ae165831e93"
    );
    assert_eq!(proof.address, address);
    assert_eq!(proof.proof_type, "bip322_simple");
    assert_eq!(proof.proof_strength, "standard");
}

#[test]
fn bitcoin_signed_message_p2pkh_verifies() {
    let signing_key = bip322_test_signing_key();
    let address = bitcoin_p2pkh_test_address(&signing_key);
    assert!(address.starts_with("1"));
    let public_key = bitcoin_test_public_key(&signing_key);
    let message = "Hello Legacy";
    let signature = sign_bitcoin_message(&signing_key, message);

    let proof =
        verify_bitcoin_signed_message("bitcoin", &address, message, &signature, &public_key)
            .unwrap();

    assert_eq!(proof.address, address);
    assert_eq!(proof.proof_type, "bitcoin_signed_message");
    assert_eq!(proof.proof_strength, "standard");
}

#[test]
fn bitcoin_signed_message_p2shwpkh_verifies() {
    let signing_key = bip322_test_signing_key();
    let address = bitcoin_p2shwpkh_test_address(&signing_key);
    assert!(address.starts_with("3"));
    let public_key = bitcoin_test_public_key(&signing_key);
    let message = "Hello Nested";
    let signature = sign_bitcoin_message(&signing_key, message);

    let proof =
        verify_bitcoin_signed_message("bitcoin", &address, message, &signature, &public_key)
            .unwrap();

    assert_eq!(proof.address, address);
    assert_eq!(proof.proof_type, "bitcoin_signed_message");
    assert_eq!(proof.proof_strength, "standard");
}

#[test]
fn bitcoin_signed_message_rejects_mismatched_public_key() {
    let signing_key = bip322_test_signing_key();
    let wrong_key = SigningKey::from_slice(&[9_u8; 32]).unwrap();
    let address = bitcoin_p2pkh_test_address(&signing_key);
    let message = "Hello Legacy";
    let signature = sign_bitcoin_message(&signing_key, message);

    let err = verify_bitcoin_signed_message(
        "bitcoin",
        &address,
        message,
        &signature,
        &bitcoin_test_public_key(&wrong_key),
    )
    .unwrap_err();

    assert!(err.contains("public key does not match"));
}

#[test]
fn challenge_and_verify_bitcoin_bip322_proof() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let signing_key = bip322_test_signing_key();
    let address = bip322_test_address(&signing_key);

    let challenge = provider.handle(Request::BitcoinChallenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address: address.clone(),
        network: "bitcoin".into(),
        resources: vec!["elastos://wallet/account/link".into()],
    });
    let message = match challenge {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["schema"], BITCOIN_CHALLENGE_SCHEMA);
            assert_eq!(data["proof_type"], "bip322_simple");
            data["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected Bitcoin challenge, got {other:?}"),
    };
    let signature = sign_bip322_simple_p2wpkh(&signing_key, &address, &message);

    match provider.handle(Request::VerifyBip322Proof {
        message,
        signature,
        signature_type: None,
        public_key: None,
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["schema"], "elastos.wallet.proof/v1");
            assert_eq!(data["proof_type"], "bip322_simple");
            assert_eq!(data["proof_strength"], "standard");
            assert_eq!(
                data["chain_namespace"],
                "bip122:000000000019d6689c085ae165831e93"
            );
            assert_eq!(data["address"], address);
            assert!(data["message_hash"].as_str().unwrap().starts_with("0x"));
        }
        other => panic!("expected verified Bitcoin proof, got {other:?}"),
    }
}

#[test]
fn challenge_and_verify_bitcoin_taproot_bip322_proof() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let secret_key = bip322_test_taproot_secret_key();
    let address = bip322_test_taproot_address(&secret_key);

    let challenge = provider.handle(Request::BitcoinChallenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address: address.clone(),
        network: "bitcoin".into(),
        resources: vec!["elastos://wallet/account/link".into()],
    });
    let message = match challenge {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["schema"], BITCOIN_CHALLENGE_SCHEMA);
            assert_eq!(data["proof_type"], "bip322_simple");
            data["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected Bitcoin challenge, got {other:?}"),
    };
    let signature = sign_bip322_simple_p2tr(&secret_key, &address, &message);

    match provider.handle(Request::VerifyBip322Proof {
        message,
        signature,
        signature_type: None,
        public_key: None,
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["schema"], "elastos.wallet.proof/v1");
            assert_eq!(data["proof_type"], "bip322_simple");
            assert_eq!(data["proof_strength"], "standard");
            assert_eq!(
                data["chain_namespace"],
                "bip122:000000000019d6689c085ae165831e93"
            );
            assert_eq!(data["address"], address);
        }
        other => panic!("expected verified Taproot Bitcoin proof, got {other:?}"),
    }
}

#[test]
fn challenge_and_verify_bitcoin_legacy_signed_message_proof() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let signing_key = bip322_test_signing_key();
    let address = bitcoin_p2pkh_test_address(&signing_key);
    let public_key = bitcoin_test_public_key(&signing_key);

    let challenge = provider.handle(Request::BitcoinChallenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address: address.clone(),
        network: "bitcoin".into(),
        resources: vec!["elastos://wallet/account/link".into()],
    });
    let message = match challenge {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["schema"], BITCOIN_CHALLENGE_SCHEMA);
            assert_eq!(data["proof_type"], "bitcoin_signed_message");
            data["message"].as_str().unwrap().to_string()
        }
        other => panic!("expected Bitcoin challenge, got {other:?}"),
    };
    let signature = sign_bitcoin_message(&signing_key, &message);

    match provider.handle(Request::VerifyBip322Proof {
        message,
        signature,
        signature_type: Some("bitcoin_signed_message".into()),
        public_key: Some(public_key),
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["schema"], "elastos.wallet.proof/v1");
            assert_eq!(data["proof_type"], "bitcoin_signed_message");
            assert_eq!(data["address"], address);
        }
        other => panic!("expected verified legacy Bitcoin proof, got {other:?}"),
    }
}

#[test]
fn bitcoin_bip322_challenge_rejects_unsupported_p2wsh_script() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let address = "bc1qp0ahvfh83088w49k405szqgg4f3pptr7p2g06tdxfjcd40z4lh4q95lsz9";

    match provider.handle(Request::BitcoinChallenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address: address.into(),
        network: "bitcoin".into(),
        resources: vec![],
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("unsupported Bitcoin address type"));
        }
        other => panic!("expected unsupported script rejection, got {other:?}"),
    }
}

#[test]
fn challenge_and_verify_erc1271_contract_proof() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let contract = "0x00000000000000000000000000000000000000cc";
    let signature = "0x01020304";

    let challenge = provider.handle(Request::Challenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address: contract.into(),
        chain_id: 20,
        resources: vec!["elastos://wallet/account/link".into()],
    });
    let message = match challenge {
        Response::Ok { data: Some(data) } => data["message"].as_str().unwrap().to_string(),
        other => panic!("expected challenge, got {other:?}"),
    };
    let proof = erc1271_proof(&message, signature, contract, true);
    match provider.handle(Request::VerifyContractProof {
        message,
        signature: signature.into(),
        erc1271_proof: proof,
    }) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["proof_type"], "siwe_erc1271");
            assert_eq!(data["chain_namespace"], "eip155:20");
            assert_eq!(data["address"], normalize_evm_address(contract));
            assert_eq!(
                data["proof_binding_id"],
                format!("proof:wallet:eip155:20:{}", normalize_evm_address(contract))
            );
        }
        other => panic!("expected ERC-1271 proof, got {other:?}"),
    }
}

#[test]
fn erc1271_contract_proof_fails_closed_without_consuming_challenge() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let contract = "0x00000000000000000000000000000000000000cc";
    let signature = "0x01020304";

    let challenge = provider.handle(Request::Challenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address: contract.into(),
        chain_id: 20,
        resources: vec![],
    });
    let message = match challenge {
        Response::Ok { data: Some(data) } => data["message"].as_str().unwrap().to_string(),
        other => panic!("expected challenge, got {other:?}"),
    };
    match provider.handle(Request::VerifyContractProof {
        message: message.clone(),
        signature: signature.into(),
        erc1271_proof: erc1271_proof(&message, signature, contract, false),
    }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_contract_proof");
            assert!(message.contains("not accepted"));
        }
        other => panic!("expected invalid ERC-1271 proof, got {other:?}"),
    }
    match provider.handle(Request::VerifyContractProof {
        message,
        signature: signature.into(),
        erc1271_proof: erc1271_proof(
            &provider.store.challenges[0].challenge.siwe_message(),
            signature,
            contract,
            true,
        ),
    }) {
        Response::Ok { .. } => {}
        other => panic!("expected valid retry after failed proof, got {other:?}"),
    }
}

#[test]
fn proof_challenge_is_single_use() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let signing_key = SigningKey::from_bytes((&[4u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);

    let challenge = provider.handle(Request::Challenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address,
        chain_id: 20,
        resources: vec![],
    });
    let message = match challenge {
        Response::Ok { data: Some(data) } => data["message"].as_str().unwrap().to_string(),
        other => panic!("expected challenge, got {other:?}"),
    };
    let signature = sign_message(&signing_key, &message);

    assert!(matches!(
        provider.handle(Request::VerifyProof {
            message: message.clone(),
            signature: signature.clone(),
        }),
        Response::Ok { .. }
    ));
    match provider.handle(Request::VerifyProof { message, signature }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_proof");
            assert!(message.contains("already consumed"));
        }
        other => panic!("expected replay rejection, got {other:?}"),
    }
}

#[test]
fn proof_challenge_rejects_tampered_chain() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let signing_key = SigningKey::from_bytes((&[5u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);

    let challenge = provider.handle(Request::Challenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address,
        chain_id: 20,
        resources: vec![],
    });
    let mut message = match challenge {
        Response::Ok { data: Some(data) } => data["message"].as_str().unwrap().to_string(),
        other => panic!("expected challenge, got {other:?}"),
    };
    message = message.replace("Chain ID: 20", "Chain ID: 8453");
    let signature = sign_message(&signing_key, &message);

    match provider.handle(Request::VerifyProof { message, signature }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_proof");
            assert!(message.contains("chain ID") || message.contains("does not match"));
        }
        other => panic!("expected invalid proof, got {other:?}"),
    }
}

#[test]
fn proof_challenge_rejects_expired_challenge() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let signing_key = SigningKey::from_bytes((&[7u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);

    let challenge = provider.handle(Request::Challenge {
        domain: "elastos.local".into(),
        uri: "http://elastos.local/apps/home/".into(),
        address,
        chain_id: 20,
        resources: vec![],
    });
    let message = match challenge {
        Response::Ok { data: Some(data) } => data["message"].as_str().unwrap().to_string(),
        other => panic!("expected challenge, got {other:?}"),
    };
    let signature = sign_message(&signing_key, &message);
    provider.store.challenges[0].challenge.expires_at = now_ts().saturating_sub(1);

    match provider.handle(Request::VerifyProof { message, signature }) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_proof");
            assert!(message.contains("expired"));
        }
        other => panic!("expected expired proof rejection, got {other:?}"),
    }
}

#[test]
fn proof_challenge_rejects_invalid_runtime_origin() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let signing_key = SigningKey::from_bytes((&[6u8; 32]).into()).unwrap();
    let address = test_address(&signing_key);

    match provider.handle(Request::Challenge {
        domain: "evil.example/path".into(),
        uri: "https://elastos.local/apps/home/".into(),
        address,
        chain_id: 20,
        resources: vec![],
    }) {
        Response::Error { code, .. } => assert_eq!(code, "invalid_request"),
        other => panic!("expected invalid origin rejection, got {other:?}"),
    }
}

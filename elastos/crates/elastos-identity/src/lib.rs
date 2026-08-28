//! Identity management for ElastOS
//!
//! Two identity primitives:
//! - **Device DID** (`did:key:z6Mk...`) — the local Carrier/node identity.
//!   Derived from device key via `derive_did()`. Used for node signing and
//!   peer transport, not as the human account root.
//! - **WebAuthn/Passkey** — local user authentication. Credentials encrypted
//!   with device key via AES-256-GCM.

pub mod store;
pub mod webauthn;

pub use store::{
    decode_did_key, derive_did, device_key_path, encode_did_key, encode_signing_key_did,
    load_nickname, load_nickname_with_device_key, load_or_create_device_key, load_or_create_did,
    save_nickname, save_nickname_with_device_key, validate_canonical_ed25519_verifying_key_bytes,
    validate_nickname, IdentityData, IdentityStore, StoredCredential,
};
pub use webauthn::{
    AuthenticationOutcome, AuthenticationResponse, AuthenticatorAssertionResponse,
    AuthenticatorAttestationResponse, AuthenticatorSelection, CreationOptions,
    CredentialDescriptor, IdentityManager, IdentityStatus, PubKeyCredParam,
    PublicKeyCredentialCreationOptions, PublicKeyCredentialRequestOptions, RegistrationOutcome,
    RegistrationResponse, RelyingParty, RequestOptions, UserEntity,
};

//! Identity management for ElastOS
//!
//! Two identity primitives:
//! - **DID** (`did:key:z6Mk...`) — the Carrier identity. Derived from device key
//!   via `derive_did()`. Used for P2P, signing, and external identity.
//! - **WebAuthn/Passkey** — local user authentication. Credentials encrypted
//!   with device key via AES-256-GCM.

pub mod store;
pub mod webauthn;

pub use store::{
    derive_did, encode_did_key, load_or_create_device_key, load_or_create_did, IdentityData,
    IdentityStore, StoredCredential, MULTICODEC_ED25519_PUB,
};
pub use webauthn::{
    AuthenticationResponse, AuthenticatorAssertionResponse, AuthenticatorAttestationResponse,
    AuthenticatorSelection, CreationOptions, CredentialDescriptor, IdentityManager, IdentityStatus,
    PubKeyCredParam, PublicKeyCredentialCreationOptions, PublicKeyCredentialRequestOptions,
    RegistrationResponse, RelyingParty, RequestOptions, UserEntity,
};

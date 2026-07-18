use std::{fmt::Write as _, sync::Arc};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use kvmflow_domain::EndpointRole;
use kvmflow_protocol::ProtocolMessage;
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const IDENTITY_TRANSCRIPT_LABEL: &[u8] = b"kvmflow-identity-v1";

#[derive(Clone)]
pub struct DeviceIdentity {
    role: EndpointRole,
    device_name: String,
    signing_key: Arc<SigningKey>,
}

impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceIdentity")
            .field("role", &self.role)
            .field("device_name", &self.device_name)
            .field("device_id", &self.device_id())
            .finish_non_exhaustive()
    }
}

impl DeviceIdentity {
    pub fn generate(role: EndpointRole, device_name: impl Into<String>) -> Self {
        Self {
            role,
            device_name: device_name.into(),
            signing_key: Arc::new(SigningKey::generate(&mut OsRng)),
        }
    }

    pub fn from_secret_bytes(
        role: EndpointRole,
        device_name: impl Into<String>,
        secret: &[u8],
    ) -> Result<Self, IdentityError> {
        let secret: [u8; 32] = secret
            .try_into()
            .map_err(|_| IdentityError::InvalidSecretLength(secret.len()))?;
        Ok(Self {
            role,
            device_name: device_name.into(),
            signing_key: Arc::new(SigningKey::from_bytes(&secret)),
        })
    }

    pub fn role(&self) -> EndpointRole {
        self.role
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn device_id(&self) -> String {
        device_id_for_public_key(&self.public_key_bytes())
    }

    pub(crate) fn create_hello(
        &self,
        tls_exporter: &[u8],
        paired_peer_id: Option<&str>,
    ) -> (ProtocolMessage, Vec<u8>) {
        let mut nonce = vec![0; 32];
        OsRng.fill_bytes(&mut nonce);
        let public_key = self.public_key_bytes().to_vec();
        let device_id = self.device_id();
        let transcript = identity_transcript(
            tls_exporter,
            self.role,
            &device_id,
            &self.device_name,
            &public_key,
            &nonce,
            paired_peer_id,
        );
        let signature = self.signing_key.sign(&transcript).to_bytes().to_vec();

        (
            ProtocolMessage::IdentityHello {
                role: self.role,
                device_id,
                device_name: self.device_name.clone(),
                public_key,
                nonce: nonce.clone(),
                signature,
                paired_peer_id: paired_peer_id.map(str::to_owned),
            },
            nonce,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedPeer {
    pub role: EndpointRole,
    pub device_id: String,
    pub device_name: String,
    pub public_key: Vec<u8>,
}

impl TrustedPeer {
    pub fn from_identity(identity: &DeviceIdentity) -> Self {
        Self {
            role: identity.role(),
            device_id: identity.device_id(),
            device_name: identity.device_name().to_owned(),
            public_key: identity.public_key_bytes().to_vec(),
        }
    }

    pub(crate) fn matches_hello(&self, hello: &VerifiedHello) -> bool {
        self.role == hello.role
            && self.device_id == hello.device_id
            && self.public_key == hello.public_key
    }
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedHello {
    pub role: EndpointRole,
    pub device_id: String,
    pub device_name: String,
    pub public_key: Vec<u8>,
    pub nonce: Vec<u8>,
    pub paired_peer_id: Option<String>,
}

pub(crate) fn verify_hello(
    message: ProtocolMessage,
    tls_exporter: &[u8],
    expected_role: EndpointRole,
) -> Result<VerifiedHello, IdentityError> {
    let ProtocolMessage::IdentityHello {
        role,
        device_id,
        device_name,
        public_key,
        nonce,
        signature,
        paired_peer_id,
    } = message
    else {
        return Err(IdentityError::ExpectedHello);
    };
    if role != expected_role {
        return Err(IdentityError::UnexpectedRole {
            expected: expected_role,
            received: role,
        });
    }
    let public_key_bytes: [u8; 32] = public_key
        .as_slice()
        .try_into()
        .map_err(|_| IdentityError::InvalidPublicKeyLength(public_key.len()))?;
    if device_id_for_public_key(&public_key_bytes) != device_id {
        return Err(IdentityError::DeviceIdMismatch);
    }
    let verifying_key =
        VerifyingKey::from_bytes(&public_key_bytes).map_err(|_| IdentityError::InvalidPublicKey)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| IdentityError::InvalidSignature)?;
    let transcript = identity_transcript(
        tls_exporter,
        role,
        &device_id,
        &device_name,
        &public_key,
        &nonce,
        paired_peer_id.as_deref(),
    );
    verifying_key
        .verify(&transcript, &signature)
        .map_err(|_| IdentityError::InvalidSignature)?;

    Ok(VerifiedHello {
        role,
        device_id,
        device_name,
        public_key,
        nonce,
        paired_peer_id,
    })
}

fn identity_transcript(
    tls_exporter: &[u8],
    role: EndpointRole,
    device_id: &str,
    device_name: &str,
    public_key: &[u8],
    nonce: &[u8],
    paired_peer_id: Option<&str>,
) -> Vec<u8> {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(IDENTITY_TRANSCRIPT_LABEL);
    append_field(&mut transcript, tls_exporter);
    append_field(&mut transcript, format!("{role:?}").as_bytes());
    append_field(&mut transcript, device_id.as_bytes());
    append_field(&mut transcript, device_name.as_bytes());
    append_field(&mut transcript, public_key);
    append_field(&mut transcript, nonce);
    append_field(
        &mut transcript,
        paired_peer_id.unwrap_or_default().as_bytes(),
    );
    transcript
}

fn append_field(target: &mut Vec<u8>, field: &[u8]) {
    target.extend_from_slice(&(field.len() as u32).to_be_bytes());
    target.extend_from_slice(field);
}

fn device_id_for_public_key(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    let mut encoded = String::with_capacity(32);
    for byte in &digest[..16] {
        write!(&mut encoded, "{byte:02x}").expect("write to string");
    }
    encoded
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("identity secret has length {0}; expected 32 bytes")]
    InvalidSecretLength(usize),
    #[error("expected an identity hello")]
    ExpectedHello,
    #[error("identity public key has length {0}; expected 32 bytes")]
    InvalidPublicKeyLength(usize),
    #[error("identity public key is invalid")]
    InvalidPublicKey,
    #[error("identity signature is invalid")]
    InvalidSignature,
    #[error("identity device ID does not match its public key")]
    DeviceIdMismatch,
    #[error("expected role {expected:?}, received {received:?}")]
    UnexpectedRole {
        expected: EndpointRole,
        received: EndpointRole,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_round_trip_preserves_device_identity() {
        let generated = DeviceIdentity::generate(EndpointRole::WindowsController, "Windows");
        let restored = DeviceIdentity::from_secret_bytes(
            EndpointRole::WindowsController,
            "Windows",
            &generated.secret_bytes(),
        )
        .expect("restore identity");

        assert_eq!(restored.device_id(), generated.device_id());
        assert_eq!(restored.public_key_bytes(), generated.public_key_bytes());
    }

    #[test]
    fn exporter_bound_hello_rejects_a_different_tls_connection() {
        let identity = DeviceIdentity::generate(EndpointRole::MacEndpoint, "Mac");
        let (hello, _) = identity.create_hello(b"first-exporter", None);

        assert!(matches!(
            verify_hello(hello, b"second-exporter", EndpointRole::MacEndpoint),
            Err(IdentityError::InvalidSignature)
        ));
    }
}

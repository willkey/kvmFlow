use std::{net::SocketAddr, sync::Arc};

use kvmflow_domain::EndpointRole;
use kvmflow_protocol::{ProtocolEnvelope, derive_pairing_code, read_envelope, write_envelope};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::{
    ClientConfig, RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
    version::TLS13,
};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::{
    ChannelTrust, DeviceIdentity, IdentityError, SecureChannel, TrustedPeer,
    identity::{VerifiedHello, verify_hello},
};

const PAIR_ALPN: &[u8] = b"kvmflow-pair/1";
const SESSION_ALPN: &[u8] = b"kvmflow-session/1";
const TLS_SERVER_NAME: &str = "kvmflow.local";
const EXPORTER_LABEL: &[u8] = b"EXPORTER-kvmflow-identity-v1";
const MAX_BOOTSTRAP_CERT_BYTES: usize = 64 * 1024;

pub struct SecureListener {
    listener: TcpListener,
    tls_acceptor: TlsAcceptor,
    certificate: CertificateDer<'static>,
    identity: DeviceIdentity,
    trusted_peer: Option<TrustedPeer>,
}

impl std::fmt::Debug for SecureListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecureListener")
            .field("local_addr", &self.listener.local_addr())
            .field("identity", &self.identity)
            .field("trusted_peer", &self.trusted_peer)
            .finish_non_exhaustive()
    }
}

impl SecureListener {
    pub async fn bind(
        address: SocketAddr,
        identity: DeviceIdentity,
        trusted_peer: Option<TrustedPeer>,
    ) -> Result<Self, TransportError> {
        if identity.role() != EndpointRole::MacEndpoint {
            return Err(TransportError::InvalidLocalRole);
        }
        let listener = TcpListener::bind(address).await?;
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec![TLS_SERVER_NAME.to_owned()])?;
        let certificate = cert.der().clone();
        let private_key =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
        let mut config = ServerConfig::builder_with_protocol_versions(&[&TLS13])
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], private_key)?;
        config.alpn_protocols = vec![SESSION_ALPN.to_vec(), PAIR_ALPN.to_vec()];

        Ok(Self {
            listener,
            tls_acceptor: TlsAcceptor::from(Arc::new(config)),
            certificate,
            identity,
            trusted_peer,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.listener.local_addr()?)
    }

    pub fn set_trusted_peer(&mut self, trusted_peer: Option<TrustedPeer>) {
        self.trusted_peer = trusted_peer;
    }

    pub async fn accept(&self) -> Result<(SecureChannel, SocketAddr), TransportError> {
        let (mut stream, remote_addr) = self.listener.accept().await?;
        write_bootstrap_certificate(&mut stream, &self.certificate).await?;
        let tls_stream = self.tls_acceptor.accept(stream).await?;
        let channel = establish_server(
            tls_stream.into(),
            &self.identity,
            self.trusted_peer.as_ref(),
        )
        .await?;
        Ok((channel, remote_addr))
    }
}

pub async fn connect(
    address: SocketAddr,
    identity: &DeviceIdentity,
    trusted_peer: Option<&TrustedPeer>,
) -> Result<SecureChannel, TransportError> {
    if identity.role() != EndpointRole::WindowsController {
        return Err(TransportError::InvalidLocalRole);
    }
    let mut stream = TcpStream::connect(address).await?;
    stream.set_nodelay(true)?;
    let certificate = read_bootstrap_certificate(&mut stream).await?;
    let mut roots = RootCertStore::empty();
    roots
        .add(certificate)
        .map_err(|error| TransportError::InvalidBootstrapCertificate(error.to_string()))?;
    let mut config = ClientConfig::builder_with_protocol_versions(&[&TLS13])
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![if trusted_peer.is_some() {
        SESSION_ALPN.to_vec()
    } else {
        PAIR_ALPN.to_vec()
    }];
    let connector = TlsConnector::from(Arc::new(config));
    let server_name = ServerName::try_from(TLS_SERVER_NAME.to_owned())
        .map_err(|_| TransportError::InvalidServerName)?;
    let tls_stream = connector.connect(server_name, stream).await?;
    establish_client(tls_stream.into(), identity, trusted_peer).await
}

async fn establish_client(
    mut stream: TlsStream<TcpStream>,
    identity: &DeviceIdentity,
    trusted_peer: Option<&TrustedPeer>,
) -> Result<SecureChannel, TransportError> {
    let exporter = tls_exporter(&stream)?;
    let (hello, local_nonce) =
        identity.create_hello(&exporter, trusted_peer.map(|peer| peer.device_id.as_str()));
    write_envelope(&mut stream, &ProtocolEnvelope::new("handshake", 0, hello)).await?;
    let remote = read_hello(
        &mut stream,
        &exporter,
        EndpointRole::MacEndpoint,
        "handshake",
    )
    .await?;
    finish_establishment(
        stream,
        identity,
        trusted_peer,
        remote,
        local_nonce,
        exporter,
    )
}

async fn establish_server(
    mut stream: TlsStream<TcpStream>,
    identity: &DeviceIdentity,
    trusted_peer: Option<&TrustedPeer>,
) -> Result<SecureChannel, TransportError> {
    let exporter = tls_exporter(&stream)?;
    let remote = read_hello(
        &mut stream,
        &exporter,
        EndpointRole::WindowsController,
        "handshake",
    )
    .await?;
    let (hello, local_nonce) =
        identity.create_hello(&exporter, trusted_peer.map(|peer| peer.device_id.as_str()));
    write_envelope(&mut stream, &ProtocolEnvelope::new("handshake", 0, hello)).await?;
    finish_establishment(
        stream,
        identity,
        trusted_peer,
        remote,
        local_nonce,
        exporter,
    )
}

async fn read_hello(
    stream: &mut TlsStream<TcpStream>,
    exporter: &[u8],
    expected_role: EndpointRole,
    expected_session_id: &str,
) -> Result<VerifiedHello, TransportError> {
    let envelope = read_envelope(stream).await?;
    if envelope.session_id != expected_session_id || envelope.sequence != 0 {
        return Err(TransportError::InvalidHandshakeEnvelope);
    }
    Ok(verify_hello(envelope.message, exporter, expected_role)?)
}

fn finish_establishment(
    stream: TlsStream<TcpStream>,
    identity: &DeviceIdentity,
    trusted_peer: Option<&TrustedPeer>,
    remote: VerifiedHello,
    local_nonce: Vec<u8>,
    exporter: [u8; 32],
) -> Result<SecureChannel, TransportError> {
    let selected_alpn = stream
        .get_ref()
        .1
        .alpn_protocol()
        .ok_or(TransportError::MissingAlpn)?;
    let trust = match trusted_peer {
        Some(peer)
            if peer.matches_hello(&remote)
                && remote.paired_peer_id.as_deref() == Some(identity.device_id().as_str())
                && selected_alpn == SESSION_ALPN =>
        {
            ChannelTrust::Authenticated
        }
        None if remote.paired_peer_id.is_none() && selected_alpn == PAIR_ALPN => {
            ChannelTrust::Pairing {
                code: pairing_code(identity, &remote, &local_nonce, &exporter),
            }
        }
        _ => return Err(TransportError::TrustMismatch),
    };
    let session_id = derive_session_id(identity, &remote, &local_nonce, &exporter);
    let peer = TrustedPeer {
        role: remote.role,
        device_id: remote.device_id,
        device_name: remote.device_name,
        public_key: remote.public_key,
    };
    Ok(SecureChannel::new(stream, session_id, peer, trust))
}

fn pairing_code(
    identity: &DeviceIdentity,
    remote: &VerifiedHello,
    local_nonce: &[u8],
    exporter: &[u8],
) -> String {
    match identity.role() {
        EndpointRole::WindowsController => derive_pairing_code(
            exporter,
            &identity.public_key_bytes(),
            &remote.public_key,
            local_nonce,
            &remote.nonce,
        ),
        EndpointRole::MacEndpoint => derive_pairing_code(
            exporter,
            &remote.public_key,
            &identity.public_key_bytes(),
            &remote.nonce,
            local_nonce,
        ),
    }
}

fn derive_session_id(
    identity: &DeviceIdentity,
    remote: &VerifiedHello,
    local_nonce: &[u8],
    exporter: &[u8],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"kvmflow-session-id-v1");
    digest.update(exporter);
    match identity.role() {
        EndpointRole::WindowsController => {
            digest.update(identity.public_key_bytes());
            digest.update(&remote.public_key);
            digest.update(local_nonce);
            digest.update(&remote.nonce);
        }
        EndpointRole::MacEndpoint => {
            digest.update(&remote.public_key);
            digest.update(identity.public_key_bytes());
            digest.update(&remote.nonce);
            digest.update(local_nonce);
        }
    }
    let digest = digest.finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn tls_exporter(stream: &TlsStream<TcpStream>) -> Result<[u8; 32], TransportError> {
    match stream {
        TlsStream::Client(stream) => {
            stream
                .get_ref()
                .1
                .export_keying_material([0; 32], EXPORTER_LABEL, None)
        }
        TlsStream::Server(stream) => {
            stream
                .get_ref()
                .1
                .export_keying_material([0; 32], EXPORTER_LABEL, None)
        }
    }
    .map_err(TransportError::Tls)
}

async fn write_bootstrap_certificate(
    stream: &mut TcpStream,
    certificate: &CertificateDer<'_>,
) -> Result<(), TransportError> {
    let length = certificate.len();
    if length > MAX_BOOTSTRAP_CERT_BYTES {
        return Err(TransportError::BootstrapCertificateTooLarge(length));
    }
    stream.write_u32(length as u32).await?;
    stream.write_all(certificate.as_ref()).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_bootstrap_certificate(
    stream: &mut TcpStream,
) -> Result<CertificateDer<'static>, TransportError> {
    let length = stream.read_u32().await? as usize;
    if length > MAX_BOOTSTRAP_CERT_BYTES {
        return Err(TransportError::BootstrapCertificateTooLarge(length));
    }
    let mut certificate = vec![0; length];
    stream.read_exact(&mut certificate).await?;
    Ok(CertificateDer::from(certificate))
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS failed: {0}")]
    Tls(#[from] rustls::Error),
    #[error("TLS certificate generation failed: {0}")]
    CertificateGeneration(#[from] rcgen::Error),
    #[error("protocol failed: {0}")]
    Protocol(#[from] kvmflow_protocol::ProtocolError),
    #[error("identity verification failed: {0}")]
    Identity(#[from] IdentityError),
    #[error("the local device role does not match this transport endpoint")]
    InvalidLocalRole,
    #[error("TLS bootstrap certificate is {0} bytes and exceeds the limit")]
    BootstrapCertificateTooLarge(usize),
    #[error("TLS bootstrap certificate is invalid: {0}")]
    InvalidBootstrapCertificate(String),
    #[error("the fixed TLS server name is invalid")]
    InvalidServerName,
    #[error("the TLS peer did not negotiate a kvmFlow ALPN")]
    MissingAlpn,
    #[error("the identity handshake envelope is invalid")]
    InvalidHandshakeEnvelope,
    #[error("the peer identity or pairing state does not match the trusted record")]
    TrustMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvmflow_domain::{InputEvent, KeyAction, KeyUsage};
    use kvmflow_protocol::ProtocolMessage;

    #[tokio::test]
    async fn pairing_stays_on_the_tls_connection_and_forwards_input() {
        let windows = DeviceIdentity::generate(EndpointRole::WindowsController, "Test Windows");
        let mac = DeviceIdentity::generate(EndpointRole::MacEndpoint, "Test Mac");
        let listener = SecureListener::bind("127.0.0.1:0".parse().unwrap(), mac, None)
            .await
            .expect("bind listener");
        let address = listener.local_addr().expect("listener address");

        let server = tokio::spawn(async move {
            let (mut channel, _) = listener.accept().await.expect("accept channel");
            let code = match channel.trust() {
                ChannelTrust::Pairing { code } => code.clone(),
                ChannelTrust::Authenticated => panic!("expected pairing"),
            };
            assert_eq!(
                channel.receive().await.expect("client decision"),
                ProtocolMessage::PairingDecision { confirmed: true }
            );
            channel
                .send(ProtocolMessage::PairingProgress {
                    remote_confirmed: true,
                })
                .await
                .expect("send progress");
            channel
                .send(ProtocolMessage::SessionReady)
                .await
                .expect("send ready");
            let input = channel.receive().await.expect("receive input");
            (code, channel.session_id().to_owned(), input)
        });

        let mut client = connect(address, &windows, None)
            .await
            .expect("connect channel");
        let client_code = match client.trust() {
            ChannelTrust::Pairing { code } => code.clone(),
            ChannelTrust::Authenticated => panic!("expected pairing"),
        };
        client
            .send(ProtocolMessage::PairingDecision { confirmed: true })
            .await
            .expect("send decision");
        assert_eq!(
            client.receive().await.expect("pairing progress"),
            ProtocolMessage::PairingProgress {
                remote_confirmed: true
            }
        );
        assert_eq!(
            client.receive().await.expect("session ready"),
            ProtocolMessage::SessionReady
        );
        let event = InputEvent::Key {
            usage: KeyUsage::A,
            action: KeyAction::Down,
        };
        client
            .send(ProtocolMessage::Input {
                event: event.clone(),
            })
            .await
            .expect("send input");

        let (server_code, server_session_id, input) = server.await.expect("join server");
        assert_eq!(client_code, server_code);
        assert_eq!(client.session_id(), server_session_id);
        assert_eq!(input, ProtocolMessage::Input { event });
    }

    #[tokio::test]
    async fn trusted_peers_reconnect_without_a_pairing_code() {
        let windows = DeviceIdentity::generate(EndpointRole::WindowsController, "Test Windows");
        let mac = DeviceIdentity::generate(EndpointRole::MacEndpoint, "Test Mac");
        let windows_trust = TrustedPeer::from_identity(&mac);
        let mac_trust = TrustedPeer::from_identity(&windows);
        let listener = SecureListener::bind("127.0.0.1:0".parse().unwrap(), mac, Some(mac_trust))
            .await
            .expect("bind listener");
        let address = listener.local_addr().expect("listener address");

        let server = tokio::spawn(async move {
            let (channel, _) = listener.accept().await.expect("accept channel");
            assert_eq!(channel.trust(), &ChannelTrust::Authenticated);
            channel.session_id().to_owned()
        });
        let client = connect(address, &windows, Some(&windows_trust))
            .await
            .expect("connect channel");

        assert_eq!(client.trust(), &ChannelTrust::Authenticated);
        assert_eq!(
            client.session_id(),
            server.await.expect("join server").as_str()
        );
    }

    #[tokio::test]
    async fn one_sided_trust_is_rejected() {
        let windows = DeviceIdentity::generate(EndpointRole::WindowsController, "Test Windows");
        let mac = DeviceIdentity::generate(EndpointRole::MacEndpoint, "Test Mac");
        let windows_trust = TrustedPeer::from_identity(&mac);
        let listener = SecureListener::bind("127.0.0.1:0".parse().unwrap(), mac, None)
            .await
            .expect("bind listener");
        let address = listener.local_addr().expect("listener address");

        let server = tokio::spawn(async move { listener.accept().await });
        let client = connect(address, &windows, Some(&windows_trust)).await;
        let server = server.await.expect("join server");

        assert!(client.is_err() || server.is_err());
    }
}

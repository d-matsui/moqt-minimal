//! # client: MOQT client connection helper
//!
//! Provides a single `connect()` function that handles QUIC endpoint creation,
//! TLS configuration, connection establishment, and SETUP exchange.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use rustls_pki_types::CertificateDer;

use crate::quic_config;
use crate::session::MoqtSession;

/// TLS configuration for the client connection.
pub enum TlsConfig {
    /// Trust a specific certificate (for tests with self-signed certs).
    TrustCert(CertificateDer<'static>),
    /// Skip all certificate verification (development only).
    Insecure,
}

/// Connect to a MOQT relay and return a session.
///
/// Creates a QUIC client endpoint, connects to the given address,
/// and performs the SETUP exchange.
pub async fn connect(addr: SocketAddr, server_name: &str, tls: TlsConfig) -> Result<MoqtSession> {
    let client_config = match tls {
        TlsConfig::TrustCert(cert_der) => quic_config::make_client_config(cert_der),
        TlsConfig::Insecure => make_insecure_client_config(),
    };

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(client_config);

    let connection = endpoint.connect(addr, server_name)?.await?;
    let url: url::Url = format!("https://{addr}").parse()?;
    let session = web_transport_quinn::Session::raw(
        connection,
        url,
        web_transport_quinn::http::StatusCode::OK,
    );
    MoqtSession::connect(session).await
}

/// Create a QUIC client config that skips certificate verification.
/// For development only.
fn make_insecure_client_config() -> quinn::ClientConfig {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};

    #[derive(Debug)]
    struct SkipVerification;

    impl ServerCertVerifier for SkipVerification {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    let mut client_crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipVerification))
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![quic_config::ALPN_MOQT.to_vec()];

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    quinn::ClientConfig::new(Arc::new(quic_config))
}

//! # quic_config: QUIC/TLS configuration helpers
//!
//! MOQT runs over QUIC, so TLS configuration is required for connections.
//! This module provides helper functions to easily create server-side and
//! client-side QUIC configurations.
//!
//! ## ALPN (Application-Layer Protocol Negotiation)
//! During the TLS handshake, both endpoints agree on the MOQT protocol
//! by specifying "moqt-17" as the ALPN protocol ID.

use std::sync::Arc;

/// ALPN protocol identifier for MOQT draft-17 (raw QUIC).
pub const ALPN_MOQT: &[u8] = b"moqt-17";

/// ALPN protocol identifier for HTTP/3 (WebTransport).
pub const ALPN_H3: &[u8] = b"h3";

/// Create a QUIC server configuration with the given certificate and private key.
/// Used by the relay server.
pub fn make_server_config(
    cert_der: rustls_pki_types::CertificateDer<'static>,
    key_der: rustls_pki_types::PrivateKeyDer<'static>,
) -> quinn::ServerConfig {
    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("failed to build server TLS config");
    server_crypto.alpn_protocols = vec![ALPN_MOQT.to_vec(), ALPN_H3.to_vec()];

    let quic_server_config =
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto).unwrap();
    quinn::ServerConfig::with_crypto(Arc::new(quic_server_config))
}

/// Create a QUIC client configuration that trusts the given certificate.
/// Used by publishers and subscribers.
///
/// In production, CA-signed certificates are used; during development,
/// self-signed certificates are trusted via this function.
pub fn make_client_config(
    cert_der: rustls_pki_types::CertificateDer<'static>,
) -> quinn::ClientConfig {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add(cert_der).expect("failed to add cert");

    let mut client_crypto = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![ALPN_MOQT.to_vec()];

    let quic_client_config =
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    quinn::ClientConfig::new(Arc::new(quic_client_config))
}

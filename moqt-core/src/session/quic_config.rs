use std::sync::Arc;

/// ALPN protocol identifier for MOQT draft-17
pub const ALPN: &[u8] = b"moqt-17";

/// Create a quinn ServerConfig with the given certificate and key.
pub fn make_server_config(
    cert_der: rustls_pki_types::CertificateDer<'static>,
    key_der: rustls_pki_types::PrivateKeyDer<'static>,
) -> quinn::ServerConfig {
    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("failed to build server TLS config");
    server_crypto.alpn_protocols = vec![ALPN.to_vec()];

    let quic_server_config =
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto).unwrap();
    quinn::ServerConfig::with_crypto(Arc::new(quic_server_config))
}

/// Create a quinn ClientConfig that trusts the given certificate.
pub fn make_client_config(
    cert_der: rustls_pki_types::CertificateDer<'static>,
) -> quinn::ClientConfig {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add(cert_der).expect("failed to add cert");

    let mut client_crypto = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![ALPN.to_vec()];

    let quic_client_config =
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    quinn::ClientConfig::new(Arc::new(quic_client_config))
}

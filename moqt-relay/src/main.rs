use moqt_relay::relay;

use std::net::SocketAddr;

use moqt_core::session::quic_config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    let addr: SocketAddr = "0.0.0.0:4433".parse()?;

    // Generate self-signed cert
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    let cert_der = rustls_pki_types::CertificateDer::from(cert);
    let key_der = rustls_pki_types::PrivateKeyDer::Pkcs8(
        rustls_pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()),
    );

    let server_config = quic_config::make_server_config(cert_der, key_der);
    let endpoint = quinn::Endpoint::server(server_config, addr)?;

    println!("Relay listening on {addr}");
    let relay = relay::Relay::new(endpoint);
    relay.run().await?;

    Ok(())
}

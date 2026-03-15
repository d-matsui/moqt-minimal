use std::net::SocketAddr;
use std::time::Duration;

use moqt_core::data::object::ObjectHeader;
use moqt_core::data::subgroup_header::SubgroupHeader;
use moqt_core::message::publish_done::PublishDoneMessage;
use moqt_core::message::publish_namespace::PublishNamespaceMessage;
use moqt_core::message::setup::{SetupMessage, SetupOption};
use moqt_core::message::subscribe::SubscribeMessage;
use moqt_core::message::subscribe_ok::SubscribeOkMessage;
use moqt_core::session::control_stream::ControlStreamReader;
use moqt_core::wire::reason_phrase::ReasonPhrase;
use moqt_core::wire::track_namespace::TrackNamespace;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    let relay_addr: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4433".to_string())
        .parse()?;

    // For testing: accept any certificate
    let client_config = make_insecure_client_config();
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())?;
    endpoint.set_default_client_config(client_config);

    println!("Connecting to relay at {relay_addr}...");
    let connection = endpoint.connect(relay_addr, "localhost")?.await?;
    println!("Connected.");

    // SETUP exchange
    let mut ctrl_send = connection.open_uni().await?;
    let setup = SetupMessage {
        setup_options: vec![
            SetupOption::Path(b"/".to_vec()),
            SetupOption::Authority(b"localhost".to_vec()),
        ],
    };
    let mut buf = Vec::new();
    setup.encode(&mut buf);
    ctrl_send.write_all(&buf).await?;

    let recv = connection.accept_uni().await?;
    let mut reader = ControlStreamReader::new(recv);
    let _relay_setup = reader.read_setup().await?;
    println!("SETUP exchange complete.");

    // PUBLISH_NAMESPACE
    let namespace = TrackNamespace {
        fields: vec![b"example".to_vec()],
    };
    let (mut ns_send, ns_recv) = connection.open_bi().await?;
    let pub_ns = PublishNamespaceMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: namespace.clone(),
    };
    buf.clear();
    pub_ns.encode(&mut buf);
    ns_send.write_all(&buf).await?;

    let mut ns_reader = ControlStreamReader::new(ns_recv);
    let _ok = ns_reader.read_message_bytes().await?;
    println!("PUBLISH_NAMESPACE registered.");

    // Wait for SUBSCRIBE from relay
    println!("Waiting for SUBSCRIBE...");
    let (mut sub_send, sub_recv) = connection.accept_bi().await?;
    let mut sub_reader = ControlStreamReader::new(sub_recv);
    let sub_bytes = sub_reader.read_message_bytes().await?;
    let mut slice = sub_bytes.as_slice();
    let subscribe = SubscribeMessage::decode(&mut slice)?;
    println!(
        "Received SUBSCRIBE for track: {:?}",
        String::from_utf8_lossy(&subscribe.track_name)
    );

    // Respond with SUBSCRIBE_OK
    let ok = SubscribeOkMessage {
        track_alias: 1,
        parameters: vec![],
    };
    buf.clear();
    ok.encode(&mut buf);
    sub_send.write_all(&buf).await?;
    println!("Sent SUBSCRIBE_OK (alias=1).");

    // Send 5 groups, 3 objects each, with 500ms between groups
    for group_id in 0u64..5 {
        let mut uni = connection.open_uni().await?;
        let header = SubgroupHeader {
            track_alias: 1,
            group_id,
        };
        let mut data = Vec::new();
        header.encode(&mut data);

        for obj_id in 0u64..3 {
            let payload = format!("g{group_id}o{obj_id}");
            let obj = ObjectHeader {
                object_id_delta: 0,
                payload_length: payload.len() as u64,
            };
            obj.encode(&mut data);
            data.extend_from_slice(payload.as_bytes());
        }

        uni.write_all(&data).await?;
        uni.finish()?;
        println!("Sent group {group_id} (3 objects)");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // PUBLISH_DONE
    let done = PublishDoneMessage {
        status_code: 0x2, // TRACK_ENDED
        stream_count: 5,
        reason_phrase: ReasonPhrase { value: vec![] },
    };
    buf.clear();
    done.encode(&mut buf);
    sub_send.write_all(&buf).await?;
    println!("Sent PUBLISH_DONE. Exiting.");

    Ok(())
}

/// Create a client config that skips certificate verification (for testing only).
fn make_insecure_client_config() -> quinn::ClientConfig {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use std::sync::Arc;

    #[derive(Debug)]
    struct SkipVerification;

    impl ServerCertVerifier for SkipVerification {
        fn verify_server_cert(
            &self, _: &rustls_pki_types::CertificateDer<'_>, _: &[rustls_pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>, _: &[u8], _: rustls::pki_types::UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self, _: &[u8], _: &rustls_pki_types::CertificateDer<'_>, _: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self, _: &[u8], _: &rustls_pki_types::CertificateDer<'_>, _: &rustls::DigitallySignedStruct,
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
    client_crypto.alpn_protocols = vec![moqt_core::session::quic_config::ALPN.to_vec()];

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    quinn::ClientConfig::new(Arc::new(quic_config))
}

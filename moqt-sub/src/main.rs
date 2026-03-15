use std::net::SocketAddr;

use moqt_core::data::object::{resolve_object_id, ObjectHeader};
use moqt_core::data::subgroup_header::SubgroupHeader;
use moqt_core::message::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::message::publish_done::PublishDoneMessage;
use moqt_core::message::setup::{SetupMessage, SetupOption};
use moqt_core::message::subscribe::SubscribeMessage;
use moqt_core::message::subscribe_ok::SubscribeOkMessage;
use moqt_core::session::control_stream::ControlStreamReader;
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

    // SUBSCRIBE
    let (mut sub_send, sub_recv) = connection.open_bi().await?;
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![MessageParameter::SubscriptionFilter(
            SubscriptionFilter::NextGroupStart,
        )],
    };
    buf.clear();
    subscribe.encode(&mut buf);
    sub_send.write_all(&buf).await?;
    println!("Sent SUBSCRIBE.");

    // Read SUBSCRIBE_OK
    let mut sub_reader = ControlStreamReader::new(sub_recv);
    let ok_bytes = sub_reader.read_message_bytes().await?;
    let mut slice = ok_bytes.as_slice();
    let subscribe_ok = SubscribeOkMessage::decode(&mut slice)?;
    println!("Received SUBSCRIBE_OK (alias={}).", subscribe_ok.track_alias);

    // Receive Object streams
    let conn = connection.clone();
    let receive_handle = tokio::spawn(async move {
        loop {
            match conn.accept_uni().await {
                Ok(mut uni_recv) => {
                    // Read all data from the stream
                    let mut all_data = Vec::new();
                    let mut tmp = vec![0u8; 4096];
                    loop {
                        match uni_recv.read(&mut tmp).await {
                            Ok(Some(n)) => all_data.extend_from_slice(&tmp[..n]),
                            Ok(None) => break,
                            Err(e) => {
                                eprintln!("read error: {e}");
                                return;
                            }
                        }
                    }

                    if all_data.is_empty() {
                        continue;
                    }

                    // Decode
                    let mut data = all_data.as_slice();
                    match SubgroupHeader::decode(&mut data) {
                        Ok(header) => {
                            println!(
                                "  Group {} (alias={}):",
                                header.group_id, header.track_alias
                            );
                            let mut prev_id: Option<u64> = None;
                            while !data.is_empty() {
                                let obj = ObjectHeader::decode(&mut data).unwrap();
                                let id = resolve_object_id(prev_id, obj.object_id_delta);
                                let payload = &data[..obj.payload_length as usize];
                                data = &data[obj.payload_length as usize..];
                                println!(
                                    "    Object {id}: {}",
                                    String::from_utf8_lossy(payload)
                                );
                                prev_id = Some(id);
                            }
                        }
                        Err(e) => eprintln!("decode error: {e}"),
                    }
                }
                Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                Err(quinn::ConnectionError::LocallyClosed) => break,
                Err(e) => {
                    eprintln!("accept error: {e}");
                    break;
                }
            }
        }
    });

    // Wait for PUBLISH_DONE
    let done_bytes = sub_reader.read_message_bytes().await?;
    let mut done_slice = done_bytes.as_slice();
    let publish_done = PublishDoneMessage::decode(&mut done_slice)?;
    println!(
        "Received PUBLISH_DONE (status={}, streams={}).",
        publish_done.status_code, publish_done.stream_count
    );

    // Give a moment for remaining objects to arrive
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    connection.close(0u32.into(), b"done");
    let _ = receive_handle.await;

    println!("Done.");
    Ok(())
}

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

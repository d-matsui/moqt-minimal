use std::io::Read;
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
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    let args: Vec<String> = std::env::args().collect();
    let pipe_mode = args.iter().any(|a| a == "--pipe");
    let relay_addr: SocketAddr = args
        .iter()
        .find(|a| !a.starts_with('-') && a.contains(':'))
        .map(|s| s.as_str())
        .unwrap_or("127.0.0.1:4433")
        .parse()?;
    let namespace = args
        .iter()
        .position(|a| a == "--ns")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("example");
    let track_name = args
        .iter()
        .position(|a| a == "--track")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("video");

    let client_config = make_insecure_client_config();
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())?;
    endpoint.set_default_client_config(client_config);

    eprintln!("Connecting to relay at {relay_addr}...");
    let connection = endpoint.connect(relay_addr, "localhost")?.await?;
    eprintln!("Connected.");

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
    eprintln!("SETUP exchange complete.");

    // PUBLISH_NAMESPACE
    let ns = TrackNamespace {
        fields: vec![namespace.as_bytes().to_vec()],
    };
    let (mut ns_send, ns_recv) = connection.open_bi().await?;
    let pub_ns = PublishNamespaceMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: ns.clone(),
    };
    buf.clear();
    pub_ns.encode(&mut buf);
    ns_send.write_all(&buf).await?;

    let mut ns_reader = ControlStreamReader::new(ns_recv);
    let _ok = ns_reader.read_message_bytes().await?;
    eprintln!("PUBLISH_NAMESPACE registered.");

    // Wait for SUBSCRIBE
    eprintln!("Waiting for SUBSCRIBE...");
    let (mut sub_send, sub_recv) = connection.accept_bi().await?;
    let mut sub_reader = ControlStreamReader::new(sub_recv);
    let sub_bytes = sub_reader.read_message_bytes().await?;
    let mut slice = sub_bytes.as_slice();
    let subscribe = SubscribeMessage::decode(&mut slice)?;
    eprintln!(
        "Received SUBSCRIBE for track: {:?}",
        String::from_utf8_lossy(&subscribe.track_name)
    );

    let ok = SubscribeOkMessage {
        track_alias: 1,
        parameters: vec![],
    };
    buf.clear();
    ok.encode(&mut buf);
    sub_send.write_all(&buf).await?;
    eprintln!("Sent SUBSCRIBE_OK (alias=1).");

    if pipe_mode {
        // Pipe mode: read H.264 NAL units from stdin
        let conn = connection.clone();
        let stream_count = send_from_stdin(conn, track_name).await?;

        let done = PublishDoneMessage {
            status_code: 0x2,
            stream_count,
            reason_phrase: ReasonPhrase { value: vec![] },
        };
        buf.clear();
        done.encode(&mut buf);
        sub_send.write_all(&buf).await?;
        // Wait for data to be flushed before closing connection
        tokio::time::sleep(Duration::from_secs(1)).await;
        eprintln!("Sent PUBLISH_DONE ({stream_count} streams). Exiting.");
    } else {
        // Demo mode: send dummy data
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
            eprintln!("Sent group {group_id} (3 objects)");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let done = PublishDoneMessage {
            status_code: 0x2,
            stream_count: 5,
            reason_phrase: ReasonPhrase { value: vec![] },
        };
        buf.clear();
        done.encode(&mut buf);
        sub_send.write_all(&buf).await?;
        eprintln!("Sent PUBLISH_DONE. Exiting.");
    }

    Ok(())
}

/// IVF frame with keyframe flag.
struct IvfFrame {
    data: Vec<u8>,
    is_keyframe: bool,
}

/// Read VP8 frames in IVF container from stdin and send as MOQT Objects.
/// Keyframes start a new Group.
async fn send_from_stdin(conn: quinn::Connection, _track_name: &str) -> anyhow::Result<u64> {
    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel::<IvfFrame>(64);

    // Blocking reader: parse IVF container from stdin
    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();

        // Skip IVF file header (32 bytes)
        let mut file_header = [0u8; 32];
        if reader.read_exact(&mut file_header).is_err() {
            eprintln!("failed to read IVF file header");
            return;
        }

        // Read frames
        loop {
            // IVF frame header: 12 bytes
            //   bytes 0-3: frame size (little-endian u32)
            //   bytes 4-11: timestamp (little-endian u64)
            let mut frame_header = [0u8; 12];
            if reader.read_exact(&mut frame_header).is_err() {
                break; // EOF
            }
            let frame_size = u32::from_le_bytes([
                frame_header[0],
                frame_header[1],
                frame_header[2],
                frame_header[3],
            ]) as usize;

            // Read frame data
            let mut data = vec![0u8; frame_size];
            if reader.read_exact(&mut data).is_err() {
                break;
            }

            // VP8 keyframe detection: bit 0 of first byte is 0 for keyframe
            let is_keyframe = !data.is_empty() && (data[0] & 0x01) == 0;

            if frame_tx
                .blocking_send(IvfFrame { data, is_keyframe })
                .is_err()
            {
                return;
            }
        }
    });

    let mut group_id: u64 = 0;
    let mut object_id: u64 = 0;
    let mut current_stream: Option<quinn::SendStream> = None;
    let mut stream_count: u64 = 0;
    let mut group_started = false;

    while let Some(frame) = frame_rx.recv().await {
        // Start new group on keyframe
        if frame.is_keyframe && group_started {
            if let Some(mut stream) = current_stream.take() {
                stream.finish()?;
            }
            stream_count += 1;
            eprintln!("Sent group {group_id} ({object_id} objects)");
            group_id += 1;
            object_id = 0;
        }

        // Open new stream if needed
        if current_stream.is_none() {
            let mut uni = conn.open_uni().await?;
            let header = SubgroupHeader {
                track_alias: 1,
                group_id,
            };
            let mut header_buf = Vec::new();
            header.encode(&mut header_buf);
            uni.write_all(&header_buf).await?;
            current_stream = Some(uni);
            group_started = true;
        }

        // Write object: 1 VP8 frame = 1 Object
        if let Some(ref mut stream) = current_stream {
            let obj = ObjectHeader {
                object_id_delta: 0,
                payload_length: frame.data.len() as u64,
            };
            let mut obj_buf = Vec::new();
            obj.encode(&mut obj_buf);
            stream.write_all(&obj_buf).await?;
            stream.write_all(&frame.data).await?;
            object_id += 1;
        }
    }

    // Close last stream
    if let Some(mut stream) = current_stream.take() {
        stream.finish()?;
        stream_count += 1;
        eprintln!("Sent group {group_id} ({object_id} objects)");
    }

    Ok(stream_count)
}

fn make_insecure_client_config() -> quinn::ClientConfig {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use std::sync::Arc;

    #[derive(Debug)]
    struct SkipVerification;

    impl ServerCertVerifier for SkipVerification {
        fn verify_server_cert(
            &self,
            _: &rustls_pki_types::CertificateDer<'_>,
            _: &[rustls_pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &rustls_pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &rustls_pki_types::CertificateDer<'_>,
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
    client_crypto.alpn_protocols = vec![moqt_core::session::quic_config::ALPN.to_vec()];

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    quinn::ClientConfig::new(Arc::new(quic_config))
}

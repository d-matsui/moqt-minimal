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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

/// Read H.264 Annex B byte stream from stdin, split into NAL units,
/// and send as MOQT Objects in real-time. IDR frames start a new Group.
async fn send_from_stdin(
    conn: quinn::Connection,
    _track_name: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let (nal_tx, mut nal_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

    // Blocking reader: read stdin, detect NAL unit boundaries, send each NAL unit
    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();
        let mut buf = [0u8; 4096];
        let mut accum = Vec::new();

        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => n,
                Err(e) => {
                    eprintln!("stdin read error: {e}");
                    break;
                }
            };
            accum.extend_from_slice(&buf[..n]);

            // Extract complete NAL units from accum
            while let Some(end) = find_start_code(&accum, 1) {
                let nal = accum[..end].to_vec();
                accum = accum[end..].to_vec();
                if nal_tx.blocking_send(nal).is_err() {
                    return;
                }
            }
        }

        // Send remaining data as last NAL
        if !accum.is_empty() {
            let _ = nal_tx.blocking_send(accum);
        }
    });

    let mut group_id: u64 = 0;
    let mut object_id: u64 = 0;
    let mut current_stream: Option<quinn::SendStream> = None;
    let mut stream_count: u64 = 0;
    let mut group_started = false;

    while let Some(nal) = nal_rx.recv().await {
        let nal_type = nal_unit_type(&nal);
        let is_idr = nal_type == 5;

        // Start new group on IDR
        if is_idr && group_started {
            // Close current stream
            if let Some(mut stream) = current_stream.take() {
                stream.finish()?;
            }
            stream_count += 1;
            eprintln!("Sent group {group_id} ({object_id} objects)");
            group_id += 1;
            object_id = 0;
        }

        // Open new stream if needed
        if current_stream.is_none() || is_idr || !group_started {
            if current_stream.is_none() && group_started {
                // Should not happen, but handle gracefully
            }
            if is_idr || !group_started {
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
        }

        // Write object
        if let Some(ref mut stream) = current_stream {
            let obj = ObjectHeader {
                object_id_delta: 0,
                payload_length: nal.len() as u64,
            };
            let mut obj_buf = Vec::new();
            obj.encode(&mut obj_buf);
            stream.write_all(&obj_buf).await?;
            stream.write_all(&nal).await?;
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

/// Find the position of the Nth start code (0x00000001 or 0x000001) in data.
/// Returns None if not found. skip=0 finds the first, skip=1 finds the second, etc.
fn find_start_code(data: &[u8], skip: usize) -> Option<usize> {
    let mut found = 0;
    let mut i = 0;
    while i < data.len() {
        if i + 2 < data.len() && data[i] == 0 && data[i + 1] == 0 {
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                if found == skip {
                    return Some(i);
                }
                found += 1;
                i += 4;
                continue;
            } else if data[i + 2] == 1 {
                if found == skip {
                    return Some(i);
                }
                found += 1;
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Get the NAL unit type from a NAL unit (including start code).
fn nal_unit_type(nal: &[u8]) -> u8 {
    // Skip start code to find the NAL header byte
    let offset = if nal.len() > 3 && nal[0] == 0 && nal[1] == 0 && nal[2] == 0 && nal[3] == 1 {
        4
    } else if nal.len() > 2 && nal[0] == 0 && nal[1] == 0 && nal[2] == 1 {
        3
    } else {
        0
    };
    if offset < nal.len() {
        nal[offset] & 0x1f
    } else {
        0
    }
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

//! # moqt-pub: MOQT Publisher
//!
//! A client that connects to a relay server and publishes media data.
//! Supports two modes:
//!
//! ## Demo mode (default)
//! Sends dummy data (strings) as 5 groups x 3 objects each.
//! Used for protocol behavior verification.
//!
//! ## Pipe mode (`--pipe`)
//! Reads VP8 video in IVF container format from stdin
//! and publishes it as MOQT objects. Used with ffmpeg, etc.
//!
//! ```bash
//! # Demo mode
//! cargo run --bin moqt-pub
//!
//! # Pipe mode (convert camera video to VP8/IVF with ffmpeg and publish)
//! ffmpeg -f avfoundation -i "0" -c:v libvpx -f ivf - | cargo run --bin moqt-pub -- --pipe
//! ```
//!
//! ## IVF to MOQT mapping
//! - VP8 keyframe -> start of a new Group (independently decodable unit)
//! - Each VP8 frame -> one Object (individual data within a Group)

use std::io::Read;
use std::net::SocketAddr;
use std::time::Duration;

use moqt_core::data::object::ObjectHeader;
use moqt_core::data::subgroup_header::SubgroupHeader;
use moqt_core::message::publish_done::{PublishDoneMessage, STATUS_TRACK_ENDED};
use moqt_core::message::subscribe_ok::SubscribeOkMessage;
use moqt_core::primitives::reason_phrase::ReasonPhrase;
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::data_stream::DataStreamWriter;
use moqt_core::session::moqt_session::{MoqtSession, RequestEvent};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    // Parse command-line arguments
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

    // QUIC client config that skips certificate verification (for development)
    let client_config = make_insecure_client_config();
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())?;
    endpoint.set_default_client_config(client_config);

    eprintln!("Connecting to relay at {relay_addr}...");
    let connection = endpoint.connect(relay_addr, "localhost")?.await?;
    eprintln!("Connected.");

    // === SETUP exchange ===
    let session = MoqtSession::connect(connection).await?;
    eprintln!("SETUP exchange complete.");

    // === PUBLISH_NAMESPACE ===
    let ns = TrackNamespace::from(&[namespace] as &[&str]);
    session.publish_namespace(ns.clone()).await?;
    eprintln!("PUBLISH_NAMESPACE registered.");

    // === Wait for SUBSCRIBE ===
    eprintln!("Waiting for SUBSCRIBE...");
    let mut request = match session.next_request().await? {
        RequestEvent::Subscribe(r) => r,
        _ => anyhow::bail!("expected SUBSCRIBE"),
    };
    eprintln!(
        "Received SUBSCRIBE for track: {:?}",
        String::from_utf8_lossy(&request.message.track_name)
    );

    // Send SUBSCRIBE_OK (Track Alias = 1)
    let ok = SubscribeOkMessage {
        track_alias: 1,
        parameters: vec![],
        track_properties_raw: vec![],
    };
    request.accept(&ok).await?;
    eprintln!("Sent SUBSCRIBE_OK (alias=1).");

    if pipe_mode {
        // === Pipe mode: read IVF video from stdin and publish ===
        let stream_count = send_from_stdin(&session, track_name).await?;

        // Send PUBLISH_DONE
        let done = PublishDoneMessage {
            status_code: STATUS_TRACK_ENDED,
            stream_count,
            reason_phrase: ReasonPhrase::from(""),
        };
        request.send_publish_done(&done).await?;
        // Wait for data to be flushed before closing the connection
        tokio::time::sleep(Duration::from_secs(1)).await;
        eprintln!("Sent PUBLISH_DONE ({stream_count} streams). Exiting.");
    } else {
        // === Demo mode: send dummy data ===
        for group_id in 0u64..5 {
            let header = SubgroupHeader {
                track_alias: 1,
                group_id,
                has_properties: false,
                end_of_group: true,
                subgroup_id: None,
                publisher_priority: None,
            };
            let mut data_writer = session.open_data_stream(&header).await?;

            for obj_id in 0u64..3 {
                let payload = format!("g{group_id}o{obj_id}");
                let obj = ObjectHeader {
                    object_id_delta: 0,
                    payload_length: payload.len() as u64,
                };
                data_writer.write_object(&obj, payload.as_bytes()).await?;
            }

            data_writer.finish()?;
            eprintln!("Sent group {group_id} (3 objects)");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let done = PublishDoneMessage {
            status_code: STATUS_TRACK_ENDED,
            stream_count: 5,
            reason_phrase: ReasonPhrase::from(""),
        };
        request.send_publish_done(&done).await?;
        eprintln!("Sent PUBLISH_DONE. Exiting.");
    }

    Ok(())
}

/// An IVF frame with a flag indicating whether it is a keyframe.
struct IvfFrame {
    data: Vec<u8>,
    is_keyframe: bool,
}

/// Reads VP8 video in IVF container format from stdin and sends it as MOQT objects.
///
/// ## IVF (Indeo Video Format) container structure
/// - File header: 32 bytes (signature "DKIF", codec info, resolution, etc.)
/// - Frame header: 12 bytes (frame size 4 bytes LE + timestamp 8 bytes LE)
/// - Frame data: byte sequence of the frame size
///
/// ## VP8 keyframe detection
/// If bit 0 of the first byte of the VP8 bitstream is 0, it is a keyframe.
/// Keyframes can be decoded independently, so they are used as Group boundaries.
/// This allows subscribers to start playback from any Group.
///
/// ## Mapping to MOQT
/// - Keyframe -> new Group (FIN the previous Group's stream and open a new one)
/// - Each frame -> one Object (sequential Object IDs with delta=0)
async fn send_from_stdin(session: &MoqtSession, _track_name: &str) -> anyhow::Result<u64> {
    // Bridge blocking I/O (stdin reading) and async I/O (QUIC sending) via a channel.
    // Use spawn_blocking for blocking reads, and receive frames asynchronously
    // in the main task for sending.
    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel::<IvfFrame>(64);

    // Blocking thread: parse IVF frames from stdin
    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();

        // Skip the IVF file header (32 bytes)
        let mut file_header = [0u8; 32];
        if reader.read_exact(&mut file_header).is_err() {
            eprintln!("failed to read IVF file header");
            return;
        }

        // Read frames one by one and send them to the channel
        loop {
            // IVF frame header: 12 bytes
            //   Bytes 0-3: frame size (little-endian u32)
            //   Bytes 4-11: timestamp (little-endian u64)
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

            // Read the frame data
            let mut data = vec![0u8; frame_size];
            if reader.read_exact(&mut data).is_err() {
                break;
            }

            // VP8 keyframe detection:
            // If bit 0 of the first byte of the VP8 bitstream is 0 -> keyframe
            // If bit 0 is 1 -> inter-frame (depends on previous frames)
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
    let mut current_writer: Option<DataStreamWriter> = None;
    let mut stream_count: u64 = 0;
    let mut group_started = false;

    while let Some(frame) = frame_rx.recv().await {
        // Start a new Group on each keyframe (except the very first frame)
        if frame.is_keyframe && group_started {
            if let Some(mut writer) = current_writer.take() {
                writer.finish()?;
            }
            stream_count += 1;
            eprintln!("Sent group {group_id} ({object_id} objects)");
            group_id += 1;
            object_id = 0;
        }

        // Open a new data stream if needed
        if current_writer.is_none() {
            let header = SubgroupHeader {
                track_alias: 1,
                group_id,
                has_properties: false,
                end_of_group: true,
                subgroup_id: None,
                publisher_priority: None,
            };
            current_writer = Some(session.open_data_stream(&header).await?);
            group_started = true;
        }

        // Write VP8 frame as one MOQT Object
        if let Some(ref mut writer) = current_writer {
            let obj = ObjectHeader {
                object_id_delta: 0,
                payload_length: frame.data.len() as u64,
            };
            writer.write_object(&obj, &frame.data).await?;
            object_id += 1;
        }
    }

    // Close the last Group's stream
    if let Some(mut writer) = current_writer.take() {
        writer.finish()?;
        stream_count += 1;
        eprintln!("Sent group {group_id} ({object_id} objects)");
    }

    Ok(stream_count)
}

/// Creates a QUIC client config that skips certificate verification.
/// For development only. Do not use in production.
///
/// Since the relay server uses a self-signed certificate, normal certificate
/// verification would fail. This config unconditionally trusts all certificates.
fn make_insecure_client_config() -> quinn::ClientConfig {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use std::sync::Arc;

    /// A dummy verifier that trusts all certificates.
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

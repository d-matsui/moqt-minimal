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

use moqt_core::client::{self, TlsConfig};
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::group::GroupWriter;
use moqt_core::session::moqt_session::{MoqtSession, SessionEvent};

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

    eprintln!("Connecting to relay at {relay_addr}...");
    let session = client::connect(relay_addr, "localhost", TlsConfig::Insecure).await?;
    eprintln!("Connected. SETUP exchange complete.");

    // === PUBLISH_NAMESPACE ===
    let ns = TrackNamespace::from(&[namespace] as &[&str]);
    session.publish_namespace(ns.clone()).await?;
    eprintln!("PUBLISH_NAMESPACE registered.");

    // === Wait for SUBSCRIBE ===
    eprintln!("Waiting for SUBSCRIBE...");
    let mut request = match session.next_event().await? {
        SessionEvent::Subscribe(r) => r,
        _ => anyhow::bail!("expected SUBSCRIBE"),
    };
    eprintln!(
        "Received SUBSCRIBE for track: {:?}",
        String::from_utf8_lossy(&request.message.track_name)
    );

    // Send SUBSCRIBE_OK (Track Alias = 1)
    request.accept(1).await?;
    eprintln!("Sent SUBSCRIBE_OK (alias=1).");

    if pipe_mode {
        // === Pipe mode: read IVF video from stdin and publish ===
        let stream_count = send_from_stdin(&session, track_name).await?;

        // Send PUBLISH_DONE
        request.send_publish_done(stream_count).await?;
        // Wait for data to be flushed before closing the connection
        tokio::time::sleep(Duration::from_secs(1)).await;
        eprintln!("Sent PUBLISH_DONE ({stream_count} streams). Exiting.");
    } else {
        // === Demo mode: send dummy data ===
        for group_id in 0u64..5 {
            let mut group = session.open_group(1, group_id).await?;

            for obj_id in 0u64..3 {
                let payload = format!("g{group_id}o{obj_id}");
                group.write_object(payload.as_bytes()).await?;
            }

            group.finish()?;
            eprintln!("Sent group {group_id} (3 objects)");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        request.send_publish_done(5).await?;
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
    let mut current_group: Option<GroupWriter> = None;
    let mut stream_count: u64 = 0;
    let mut group_started = false;

    while let Some(frame) = frame_rx.recv().await {
        // Start a new Group on each keyframe (except the very first frame)
        if frame.is_keyframe && group_started {
            if let Some(mut group) = current_group.take() {
                group.finish()?;
            }
            stream_count += 1;
            eprintln!("Sent group {group_id} ({object_id} objects)");
            group_id += 1;
            object_id = 0;
        }

        // Open a new group if needed
        if current_group.is_none() {
            current_group = Some(session.open_group(1, group_id).await?);
            group_started = true;
        }

        // Write VP8 frame as one MOQT Object
        if let Some(ref mut group) = current_group {
            group.write_object(&frame.data).await?;
            object_id += 1;
        }
    }

    // Close the last Group
    if let Some(mut group) = current_group.take() {
        group.finish()?;
        stream_count += 1;
        eprintln!("Sent group {group_id} ({object_id} objects)");
    }

    Ok(stream_count)
}

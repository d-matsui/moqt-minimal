//! # moqt-sub: MOQT Subscriber
//!
//! A client that connects to a relay server and receives media data.
//! Supports two modes:
//!
//! - **Default mode**: Prints received Objects in human-readable form to stderr
//! - **Pipe mode** (`--pipe`): Outputs received VP8 frames as an IVF container to stdout.
//!   Can be piped to ffplay for real-time playback:
//!   `moqt-sub --pipe | ffplay -f ivf -`
//!
//! ## Processing Flow
//! 1. Establish a QUIC connection to the relay and exchange SETUP
//! 2. Send SUBSCRIBE and receive SUBSCRIBE_OK
//! 3. Receive data on unidirectional streams
//! 4. Terminate upon receiving PUBLISH_DONE

use std::io::Write;
use std::net::SocketAddr;

use moqt_core::client::{self, TlsConfig};
use moqt_core::message::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::moqt_session::SessionEvent;

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

    if !pipe_mode {
        eprintln!("Connecting to relay at {relay_addr}...");
    }
    let session = client::connect(relay_addr, "localhost", TlsConfig::Insecure).await?;
    let connection = session.connection().clone();
    if !pipe_mode {
        eprintln!("Connected. SETUP exchange complete.");
    }

    // SUBSCRIBE
    let ns = TrackNamespace::from(&[namespace] as &[&str]);
    let mut subscription = session
        .subscribe(
            ns,
            track_name,
            vec![MessageParameter::SubscriptionFilter(
                SubscriptionFilter::NextGroupStart,
            )],
        )
        .await?;
    if !pipe_mode {
        eprintln!(
            "Received SUBSCRIBE_OK (alias={}).",
            subscription.track_alias()
        );
    }

    // Receive Object streams
    let receive_handle = tokio::spawn(async move {
        let stdout = std::io::stdout();
        let mut ivf_header_written = false;
        let mut frame_index: u64 = 0;

        loop {
            let mut group = match session.next_event().await {
                Ok(SessionEvent::DataStream(g)) => g,
                Ok(_) => continue, // ignore non-data events
                Err(_) => break,   // connection closed
            };
            if pipe_mode {
                // Pipe mode: write IVF container to stdout
                // Write IVF file header once
                if !ivf_header_written {
                    let mut ivf_hdr = [0u8; 32];
                    ivf_hdr[0..4].copy_from_slice(b"DKIF");
                    ivf_hdr[4..6].copy_from_slice(&0u16.to_le_bytes());
                    ivf_hdr[6..8].copy_from_slice(&32u16.to_le_bytes());
                    ivf_hdr[8..12].copy_from_slice(b"VP80");
                    ivf_hdr[12..14].copy_from_slice(&320u16.to_le_bytes());
                    ivf_hdr[14..16].copy_from_slice(&240u16.to_le_bytes());
                    ivf_hdr[16..20].copy_from_slice(&30u32.to_le_bytes());
                    ivf_hdr[20..24].copy_from_slice(&1u32.to_le_bytes());
                    let _ = stdout.lock().write_all(&ivf_hdr);
                    ivf_header_written = true;
                }

                // Write each Object as an IVF frame
                while let Ok(Some(payload)) = group.read_object().await {
                    let mut out = stdout.lock();
                    let size = payload.len() as u32;
                    let _ = out.write_all(&size.to_le_bytes());
                    let _ = out.write_all(&frame_index.to_le_bytes());
                    let _ = out.write_all(&payload);
                    let _ = out.flush();
                    frame_index += 1;
                }
            } else {
                // Demo mode: print human-readable
                eprintln!(
                    "  Group {} (alias={}):",
                    group.group_id(),
                    group.track_alias()
                );
                let mut object_id: u64 = 0;
                while let Ok(Some(payload)) = group.read_object().await {
                    eprintln!("    Object {object_id}: {} bytes", payload.len());
                    object_id += 1;
                }
            }
        }
    });

    // Wait for PUBLISH_DONE
    let publish_done = subscription.recv_publish_done().await?;
    if !pipe_mode {
        eprintln!(
            "Received PUBLISH_DONE (status={}, streams={}).",
            publish_done.status_code, publish_done.stream_count
        );
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    connection.close(0u32.into(), b"done");
    let _ = receive_handle.await;

    if !pipe_mode {
        eprintln!("Done.");
    }
    Ok(())
}

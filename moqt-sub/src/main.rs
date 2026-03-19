//! # moqt-sub: MOQT サブスクライバー
//!
//! リレーサーバーに接続し、メディアデータを受信するクライアント。
//! 2つのモードをサポートする:
//!
//! - **デフォルトモード**: 受信した Object を人間が読める形で stderr に表示
//! - **パイプモード** (`--pipe`): 受信した VP8 フレームを IVF コンテナとして stdout に出力
//!   ffplay にパイプすることでリアルタイム再生ができる:
//!   `moqt-sub --pipe | ffplay -f ivf -`
//!
//! ## 処理フロー
//! 1. リレーに QUIC 接続し、SETUP を交換
//! 2. SUBSCRIBE を送信して SUBSCRIBE_OK を受信
//! 3. uni ストリームでデータを受信
//! 4. PUBLISH_DONE を受信したら終了

use std::io::Write;
use std::net::SocketAddr;

use moqt_core::data::object::resolve_object_id;
use moqt_core::message::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::message::setup::{SetupMessage, SetupOption};
use moqt_core::message::subscribe::SubscribeMessage;
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::control_stream::{ControlStreamReader, ControlStreamWriter};
use moqt_core::session::data_stream::DataStreamReader;
use moqt_core::session::request_stream::{
    RequestMessage, RequestStreamReader, RequestStreamWriter,
};

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

    if !pipe_mode {
        eprintln!("Connecting to relay at {relay_addr}...");
    }
    let connection = endpoint.connect(relay_addr, "localhost")?.await?;
    if !pipe_mode {
        eprintln!("Connected.");
    }

    // SETUP exchange
    let ctrl_send = connection.open_uni().await?;
    let mut ctrl_writer = ControlStreamWriter::new(ctrl_send);
    let setup = SetupMessage {
        setup_options: vec![
            SetupOption::Path(b"/".to_vec()),
            SetupOption::Authority(b"localhost".to_vec()),
        ],
    };
    ctrl_writer.write_setup(&setup).await?;

    let recv = connection.accept_uni().await?;
    let mut reader = ControlStreamReader::new(recv);
    let _relay_setup = reader.read_setup().await?;
    if !pipe_mode {
        eprintln!("SETUP exchange complete.");
    }

    // SUBSCRIBE
    let (sub_send, sub_recv) = connection.open_bi().await?;
    let mut sub_writer = RequestStreamWriter::new(sub_send);
    let mut sub_reader = RequestStreamReader::new(sub_recv);
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![namespace.as_bytes().to_vec()],
        },
        track_name: track_name.as_bytes().to_vec(),
        parameters: vec![MessageParameter::SubscriptionFilter(
            SubscriptionFilter::NextGroupStart,
        )],
    };
    sub_writer.write_subscribe(&subscribe).await?;
    if !pipe_mode {
        eprintln!("Sent SUBSCRIBE.");
    }

    // Read SUBSCRIBE_OK
    let sub_msg = sub_reader.read_message().await?;
    let subscribe_ok = match sub_msg {
        RequestMessage::SubscribeOk(ok) => ok,
        _ => anyhow::bail!("expected SUBSCRIBE_OK"),
    };
    if !pipe_mode {
        eprintln!(
            "Received SUBSCRIBE_OK (alias={}).",
            subscribe_ok.track_alias
        );
    }

    // Receive Object streams
    let conn = connection.clone();
    let receive_handle = tokio::spawn(async move {
        let stdout = std::io::stdout();
        let mut ivf_header_written = false;
        let mut frame_index: u64 = 0;

        loop {
            match conn.accept_uni().await {
                Ok(uni_recv) => {
                    let mut data_reader = DataStreamReader::new(uni_recv);

                    let header = match data_reader.read_subgroup_header().await {
                        Ok((h, _raw)) => h,
                        Err(e) => {
                            eprintln!("decode error: {e}");
                            continue;
                        }
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
                        while let Ok(Some((_obj, payload, _raw))) = data_reader.read_object().await
                        {
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
                            header.group_id, header.track_alias
                        );
                        let mut prev_id: Option<u64> = None;
                        while let Ok(Some((obj, payload, _raw))) = data_reader.read_object().await {
                            let id = resolve_object_id(prev_id, obj.object_id_delta);
                            eprintln!("    Object {id}: {} bytes", payload.len());
                            prev_id = Some(id);
                        }
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
    let done_msg = sub_reader.read_message().await?;
    let publish_done = match done_msg {
        RequestMessage::PublishDone(done) => done,
        _ => anyhow::bail!("expected PUBLISH_DONE"),
    };
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

use std::net::SocketAddr;
use std::sync::{Arc, Once};

use moqt_core::client::{self, TlsConfig};
use moqt_core::quic_config;
use moqt_core::session::{MoqtSession, SessionEvent};
use moqt_core::wire::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::wire::track_namespace::TrackNamespace;

static INIT: Once = Once::new();

fn init_crypto() {
    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
    });
}

/// Helper: generate self-signed cert and return (cert_der, key_der)
fn gen_cert() -> (
    rustls_pki_types::CertificateDer<'static>,
    rustls_pki_types::PrivateKeyDer<'static>,
) {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_der = rustls_pki_types::CertificateDer::from(cert);
    let key_der = rustls_pki_types::PrivateKeyDer::Pkcs8(
        rustls_pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()),
    );
    (cert_der, key_der)
}

/// Helper: start relay on a random port and return the endpoint + address.
async fn start_relay() -> (
    quinn::Endpoint,
    SocketAddr,
    rustls_pki_types::CertificateDer<'static>,
) {
    let (cert_der, key_der) = gen_cert();
    let server_config = quic_config::make_server_config(cert_der.clone(), key_der);
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let endpoint = quinn::Endpoint::server(server_config, addr).unwrap();
    let local_addr = endpoint.local_addr().unwrap();
    (endpoint, local_addr, cert_der)
}

/// Helper: connect a client to the relay and do SETUP exchange.
async fn connect_client(
    relay_addr: SocketAddr,
    cert_der: rustls_pki_types::CertificateDer<'static>,
) -> MoqtSession {
    client::connect(relay_addr, "localhost", TlsConfig::TrustCert(cert_der))
        .await
        .unwrap()
}

/// Helper: send PUBLISH_NAMESPACE and receive REQUEST_OK
async fn publish_namespace(session: &MoqtSession, namespace: TrackNamespace) {
    session.publish_namespace(namespace).await.unwrap();
}

// ============================================================
// Tests
// ============================================================

/// 3.1 + 3.2: QUIC connection + SETUP exchange
#[tokio::test]
async fn session_setup() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    // Spawn relay accept loop
    let ep = endpoint.clone();
    tokio::spawn(async move {
        if let Some(incoming) = ep.accept().await {
            let conn = incoming.await.unwrap();
            let wt_session = web_transport_quinn::Session::raw(
                conn,
                url::Url::parse("https://localhost").unwrap(),
                web_transport_quinn::http::StatusCode::OK,
            );
            let _session = MoqtSession::accept(wt_session).await.unwrap();
        }
    });

    let _session = Arc::new(connect_client(addr, cert_der).await);
    // If we get here, SETUP exchange succeeded
}

/// 4.1: PUBLISH_NAMESPACE -> REQUEST_OK
#[tokio::test]
async fn publish_namespace_registration() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    // Spawn relay
    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move {
        relay.run().await.unwrap();
    });

    // Wait for relay to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let pub_session = Arc::new(connect_client(addr, cert_der).await);

    // Send PUBLISH_NAMESPACE
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;
    // If we get here, registration succeeded
}

/// 4.2: SUBSCRIBE -> SUBSCRIBE_OK (via Relay)
#[tokio::test]
async fn subscribe_via_relay() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    // Spawn relay
    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move {
        relay.run().await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher connects and registers namespace
    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    // Keep publisher connection alive for the duration of the test
    let _pub_session_keepalive = pub_session.clone();

    // Publisher: spawn task to accept SUBSCRIBE and respond with SUBSCRIBE_OK
    tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                req.accept(1).await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber connects and sends SUBSCRIBE
    let sub_session = Arc::new(connect_client(addr, cert_der).await);
    let subscription = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![MessageParameter::SubscriptionFilter(
                SubscriptionFilter::NextGroupStart,
            )],
        )
        .await
        .unwrap();

    assert_eq!(subscription.track_alias(), 1);
}

/// 5.1: Object data forwarding through Relay
#[tokio::test]
async fn object_forwarding() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move {
        relay.run().await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher setup
    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    // Keep connection alive after the spawned task completes.
    let _pub_session_keepalive = pub_session.clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                req.accept(1).await.unwrap();

                // Send a subgroup with 2 objects
                let mut group = pub_session.open_subgroup(1, 0, 0).await.unwrap();
                group.write_object(b"hello").await.unwrap();
                group.write_object(b"world").await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber setup
    let sub_session = Arc::new(connect_client(addr, cert_der).await);
    let _subscription = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    // Wait for publisher to send objects
    pub_handle.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Read forwarded objects on subscriber
    let event = sub_session.next_event().await.unwrap();
    match event {
        SessionEvent::DataStream(mut group) => {
            assert_eq!(group.track_alias(), 1);
            assert_eq!(group.group_id(), 0);

            let payload0 = group.read_object().await.unwrap().unwrap();
            assert_eq!(payload0, b"hello");

            let payload1 = group.read_object().await.unwrap().unwrap();
            assert_eq!(payload1, b"world");
        }
        _ => panic!("expected DataStream event"),
    }
}

/// 6.2: PUBLISH_DONE forwarding through Relay
#[tokio::test]
async fn publish_done_forwarding() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move {
        relay.run().await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher setup
    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    // Publisher: accept SUBSCRIBE, respond, send object, then PUBLISH_DONE
    let _pub_session_keepalive = pub_session.clone();
    tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                req.accept(1).await.unwrap();

                // Send one object
                let mut group = pub_session.open_subgroup(1, 0, 0).await.unwrap();
                group.write_object(b"done").await.unwrap();
                group.finish().unwrap();

                // Send PUBLISH_DONE on the bidi stream
                req.send_publish_done(1).await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber setup
    let sub_session = Arc::new(connect_client(addr, cert_der).await);
    let mut subscription = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    // Read PUBLISH_DONE (forwarded by relay)
    let publish_done = subscription.recv_publish_done().await.unwrap();
    assert_eq!(publish_done.status_code, 0x2); // TRACK_ENDED
    assert_eq!(publish_done.stream_count, 1);
}

/// 5.3: Multiple Groups forwarded through Relay
#[tokio::test]
async fn multiple_groups() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    // Publisher: accept SUBSCRIBE, send 3 groups with 2 objects each
    let _pub_session_keepalive = pub_session.clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                req.accept(1).await.unwrap();

                for group_id in 0u64..3 {
                    let mut group = pub_session.open_subgroup(1, group_id, 0).await.unwrap();
                    for obj_id in 0u64..2 {
                        let payload = format!("g{group_id}o{obj_id}");
                        group.write_object(payload.as_bytes()).await.unwrap();
                    }
                }

                // PUBLISH_DONE
                req.send_publish_done(3).await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber
    let sub_session = Arc::new(connect_client(addr, cert_der).await);
    let mut subscription = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    pub_handle.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Receive 3 groups
    let mut received_groups: Vec<(u64, Vec<String>)> = Vec::new();
    for _ in 0..3 {
        let event = sub_session.next_event().await.unwrap();
        match event {
            SessionEvent::DataStream(mut group) => {
                let group_id = group.group_id();
                let mut payloads = Vec::new();
                while let Some(payload) = group.read_object().await.unwrap() {
                    payloads.push(String::from_utf8(payload).unwrap());
                }
                received_groups.push((group_id, payloads));
            }
            _ => panic!("expected DataStream event"),
        }
    }

    // Sort by group_id (streams may arrive out of order)
    received_groups.sort_by_key(|(gid, _)| *gid);

    assert_eq!(received_groups.len(), 3);
    assert_eq!(
        received_groups[0],
        (0, vec!["g0o0".to_string(), "g0o1".to_string()])
    );
    assert_eq!(
        received_groups[1],
        (1, vec!["g1o0".to_string(), "g1o1".to_string()])
    );
    assert_eq!(
        received_groups[2],
        (2, vec!["g2o0".to_string(), "g2o1".to_string()])
    );

    // Verify PUBLISH_DONE
    let publish_done = subscription.recv_publish_done().await.unwrap();
    assert_eq!(publish_done.stream_count, 3);
}

/// 7.2: Late join -- Subscriber connects while Publisher is already sending
#[tokio::test]
async fn late_join() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher connects first and registers
    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    // Publisher: accept SUBSCRIBE (will come later), respond, send objects
    let _pub_session_keepalive = pub_session.clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                req.accept(1).await.unwrap();

                // Send 2 groups after subscriber joins
                for group_id in 0u64..2 {
                    let mut group = pub_session.open_subgroup(1, group_id, 0).await.unwrap();
                    let payload = format!("late-g{group_id}");
                    group.write_object(payload.as_bytes()).await.unwrap();
                }

                req.send_publish_done(2).await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber connects AFTER publisher is ready (simulating late join)
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let sub_session = Arc::new(connect_client(addr, cert_der).await);
    let _subscription = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![MessageParameter::SubscriptionFilter(
                SubscriptionFilter::NextGroupStart,
            )],
        )
        .await
        .unwrap();

    pub_handle.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Should receive objects sent after subscription
    let event = sub_session.next_event().await.unwrap();
    match event {
        SessionEvent::DataStream(mut group) => {
            assert_eq!(group.track_alias(), 1);

            let payload = group.read_object().await.unwrap().unwrap();
            let payload_str = std::str::from_utf8(&payload).unwrap();
            assert!(payload_str.starts_with("late-g"));
        }
        _ => panic!("expected DataStream event"),
    }
}

/// 3.1: ALPN mismatch -- connection should fail
#[tokio::test]
async fn alpn_mismatch() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    // Spawn relay
    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Create client with wrong ALPN
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add(cert_der).unwrap();
    let mut client_crypto = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"wrong-alpn".to_vec()];

    let quic_client_config =
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    let client_config = quinn::ClientConfig::new(std::sync::Arc::new(quic_client_config));

    let mut client_endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
    client_endpoint.set_default_client_config(client_config);

    let result = client_endpoint.connect(addr, "localhost").unwrap().await;

    assert!(result.is_err(), "connection with wrong ALPN should fail");
}

/// 4.3: SUBSCRIBE to unknown namespace -> REQUEST_ERROR
#[tokio::test]
async fn subscribe_unknown_namespace() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Subscriber connects (no publisher registered)
    let sub_session = Arc::new(connect_client(addr, cert_der).await);
    let result = sub_session
        .subscribe(
            TrackNamespace::from(["nonexistent"].as_slice()),
            "video",
            vec![],
        )
        .await;

    // session.subscribe() returns an error when REQUEST_ERROR is received
    let err = result
        .err()
        .expect("subscribe should fail for unknown namespace");
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("rejected"),
        "error should indicate rejection: {err_msg}"
    );
}

/// 6.1: Multiple subscribers receive the same objects
#[tokio::test]
async fn multiple_subscribers() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher setup
    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    // Publisher: accept 1 SUBSCRIBE (aggregation means only one arrives),
    // send objects, then PUBLISH_DONE.
    let _pub_session_keepalive = pub_session.clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        let mut req = match event {
            SessionEvent::Subscribe(req) => req,
            _ => panic!("expected Subscribe event"),
        };
        req.accept(1).await.unwrap();

        // Wait for both subscribers to be registered via aggregation
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Send 1 group with 1 object
        let mut group = pub_session.open_subgroup(1, 0, 0).await.unwrap();
        group.write_object(b"shared").await.unwrap();
        group.finish().unwrap();

        req.send_publish_done(1).await.unwrap();
    });

    // Helper to subscribe and receive objects
    async fn subscribe_and_receive(
        addr: SocketAddr,
        cert_der: rustls_pki_types::CertificateDer<'static>,
    ) -> Vec<u8> {
        let session = Arc::new(connect_client(addr, cert_der).await);
        let _subscription = session
            .subscribe(
                TrackNamespace::from(["example"].as_slice()),
                "video",
                vec![],
            )
            .await
            .unwrap();

        // Wait for objects
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let event = session.next_event().await.unwrap();
        match event {
            SessionEvent::DataStream(mut group) => group.read_object().await.unwrap().unwrap(),
            _ => panic!("expected DataStream event"),
        }
    }

    // Two subscribers connect concurrently.
    // Per-track lock ensures only one SUBSCRIBE reaches the publisher.
    let sub1 = tokio::spawn(subscribe_and_receive(addr, cert_der.clone()));
    let sub2 = tokio::spawn(subscribe_and_receive(addr, cert_der));

    pub_handle.await.unwrap();

    let payload1 = sub1.await.unwrap();
    let payload2 = sub2.await.unwrap();

    assert_eq!(payload1, b"shared");
    assert_eq!(payload2, b"shared");
}

/// 5.4: Multiple tracks -- video and audio simultaneously
#[tokio::test]
async fn multiple_tracks() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher
    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    // Publisher: accept 2 SUBSCRIBEs (video + audio), send objects on each
    let _pub_session_keepalive = pub_session.clone();
    let pub_handle = tokio::spawn(async move {
        // Accept SUBSCRIBE for video (alias=1)
        let event_v = pub_session.next_event().await.unwrap();
        let mut req_v = match event_v {
            SessionEvent::Subscribe(req) => req,
            _ => panic!("expected Subscribe event"),
        };
        req_v.accept(1).await.unwrap();

        // Accept SUBSCRIBE for audio (alias=2)
        let event_a = pub_session.next_event().await.unwrap();
        let mut req_a = match event_a {
            SessionEvent::Subscribe(req) => req,
            _ => panic!("expected Subscribe event"),
        };
        req_a.accept(2).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send video object
        let mut group_v = pub_session.open_subgroup(1, 0, 0).await.unwrap();
        group_v.write_object(b"video").await.unwrap();

        // Send audio object
        let mut group_a = pub_session.open_subgroup(2, 0, 0).await.unwrap();
        group_a.write_object(b"audio").await.unwrap();

        // PUBLISH_DONE on both
        req_v.send_publish_done(1).await.unwrap();
        req_a.send_publish_done(1).await.unwrap();
    });

    // Subscriber: subscribe to both tracks
    let sub_session = Arc::new(connect_client(addr, cert_der).await);

    // Subscribe to video
    let _sub_v = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    // Subscribe to audio
    let _sub_a = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "audio",
            vec![],
        )
        .await
        .unwrap();

    pub_handle.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Receive 2 uni streams (video + audio, order may vary)
    let mut payloads: Vec<String> = Vec::new();
    for _ in 0..2 {
        let event = sub_session.next_event().await.unwrap();
        match event {
            SessionEvent::DataStream(mut group) => {
                let payload = group.read_object().await.unwrap().unwrap();
                payloads.push(String::from_utf8(payload).unwrap());
            }
            _ => panic!("expected DataStream event"),
        }
    }

    payloads.sort();
    assert_eq!(payloads, vec!["audio", "video"]);
}

/// Subscription aggregation: second subscriber reuses the upstream subscription.
/// The publisher should receive only ONE SUBSCRIBE, and both subscribers
/// should receive the forwarded data.
#[tokio::test]
async fn subscription_aggregation() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher
    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    // Publisher: accept exactly 1 SUBSCRIBE, send data, then PUBLISH_DONE
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        let mut req = match event {
            SessionEvent::Subscribe(req) => req,
            _ => panic!("expected Subscribe event"),
        };
        req.accept(1).await.unwrap();

        // Wait for both subscribers to be registered
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Send 1 group with 1 object
        let mut group = pub_session.open_subgroup(1, 0, 0).await.unwrap();
        group.write_object(b"aggr").await.unwrap();
        group.finish().unwrap();

        req.send_publish_done(1).await.unwrap();

        // Verify no second SUBSCRIBE arrives (would timeout)
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            pub_session.next_event(),
        )
        .await;
        assert!(
            result.is_err(),
            "publisher should NOT receive a second SUBSCRIBE"
        );
    });

    // Subscriber 1: subscribe and wait for SUBSCRIBE_OK
    let sub1_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    let _sub1 = sub1_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    // Subscriber 2: subscribe AFTER sub1 is established (triggers aggregation)
    let sub2_session = Arc::new(connect_client(addr, cert_der).await);
    let _sub2 = sub2_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    // Both subscribers receive data
    let recv1 = tokio::spawn(async move {
        let event = sub1_session.next_event().await.unwrap();
        match event {
            SessionEvent::DataStream(mut group) => group.read_object().await.unwrap().unwrap(),
            _ => panic!("expected DataStream event"),
        }
    });

    let recv2 = tokio::spawn(async move {
        let event = sub2_session.next_event().await.unwrap();
        match event {
            SessionEvent::DataStream(mut group) => group.read_object().await.unwrap().unwrap(),
            _ => panic!("expected DataStream event"),
        }
    });

    pub_handle.await.unwrap();

    let payload1 = recv1.await.unwrap();
    let payload2 = recv2.await.unwrap();

    assert_eq!(payload1, b"aggr");
    assert_eq!(payload2, b"aggr");
}

/// 6.3: Subscriber disconnect -- relay cleans up, publisher continues
#[tokio::test]
async fn subscriber_disconnect() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    // Publisher: accept SUBSCRIBE, respond, send objects continuously
    let _pub_session_keepalive = pub_session.clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                req.accept(1).await.unwrap();

                // Send a few groups
                for group_id in 0u64..3 {
                    let mut group = pub_session.open_subgroup(1, group_id, 0).await.unwrap();
                    group.write_object(b"data").await.unwrap();
                    group.finish().unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }

                // Publisher session should still be usable after subscriber disconnects.
                // (Previously verified via connection().close_reason(), now
                // implicitly validated by the successful group writes above.)
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber connects, subscribes, receives 1 group, then disconnects
    {
        let sub_session = Arc::new(connect_client(addr, cert_der).await);
        let _subscription = sub_session
            .subscribe(
                TrackNamespace::from(["example"].as_slice()),
                "video",
                vec![],
            )
            .await
            .unwrap();

        // Receive at least 1 object
        let event = sub_session.next_event().await.unwrap();
        match event {
            SessionEvent::DataStream(mut group) => {
                let _ = group.read_object().await.unwrap();
            }
            _ => panic!("expected DataStream event"),
        }

        // Disconnect subscriber
        sub_session.close();
    }

    // Publisher should complete without error
    pub_handle.await.unwrap();
}

// ============================================================
// WebTransport tests
// ============================================================

/// Helper: connect a WebTransport client to the relay and do SETUP exchange.
async fn connect_webtransport_client(
    relay_addr: SocketAddr,
    cert_der: rustls_pki_types::CertificateDer<'static>,
) -> MoqtSession {
    // Build a quinn client with h3 ALPN that trusts the self-signed cert
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add(cert_der).unwrap();
    let mut client_crypto = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![web_transport_quinn::ALPN.as_bytes().to_vec()];

    let quic_client_config =
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    let client_config = quinn::ClientConfig::new(Arc::new(quic_client_config));

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(client_config);

    // QUIC connect, then WebTransport (HTTP/3 CONNECT) handshake
    let connection = endpoint
        .connect(relay_addr, "localhost")
        .unwrap()
        .await
        .expect("QUIC connect failed");
    let url = url::Url::parse(&format!("https://localhost:{}", relay_addr.port())).unwrap();
    let wt_session = web_transport_quinn::Session::connect(connection, url)
        .await
        .expect("WebTransport handshake failed");
    MoqtSession::connect(wt_session)
        .await
        .expect("MOQT SETUP failed over WebTransport")
}

/// WebTransport: SETUP exchange succeeds
#[tokio::test]
async fn webtransport_session_setup() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let _session = connect_webtransport_client(addr, cert_der).await;
    // If we get here, WebTransport SETUP exchange succeeded
}

/// WebTransport: end-to-end object forwarding (WT publisher -> relay -> WT subscriber)
#[tokio::test]
async fn webtransport_object_forwarding() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher (WebTransport)
    let pub_session = Arc::new(connect_webtransport_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    let _pub_keepalive = pub_session.clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                req.accept(1).await.unwrap();
                let mut group = pub_session.open_subgroup(1, 0, 0).await.unwrap();
                group.write_object(b"hello-wt").await.unwrap();
                group.write_object(b"world-wt").await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber (WebTransport)
    let sub_session = Arc::new(connect_webtransport_client(addr, cert_der).await);
    let _subscription = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    match sub_session.next_event().await.unwrap() {
        SessionEvent::DataStream(mut group) => {
            let obj1 = group.read_object().await.unwrap().unwrap();
            assert_eq!(obj1, b"hello-wt");
            let obj2 = group.read_object().await.unwrap().unwrap();
            assert_eq!(obj2, b"world-wt");
        }
        _ => panic!("expected DataStream event"),
    }

    pub_handle.await.unwrap();
}

/// Cross-transport: raw QUIC publisher -> relay -> WebTransport subscriber
#[tokio::test]
async fn cross_transport_quic_to_webtransport() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher (raw QUIC)
    let pub_session = Arc::new(connect_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    let _pub_keepalive = pub_session.clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                req.accept(1).await.unwrap();
                let mut group = pub_session.open_subgroup(1, 0, 0).await.unwrap();
                group.write_object(b"cross-transport").await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber (WebTransport)
    let sub_session = Arc::new(connect_webtransport_client(addr, cert_der).await);
    let _subscription = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    match sub_session.next_event().await.unwrap() {
        SessionEvent::DataStream(mut group) => {
            let obj = group.read_object().await.unwrap().unwrap();
            assert_eq!(obj, b"cross-transport");
        }
        _ => panic!("expected DataStream event"),
    }

    pub_handle.await.unwrap();
}

/// Cross-transport: WebTransport publisher -> relay -> raw QUIC subscriber
#[tokio::test]
async fn cross_transport_webtransport_to_quic() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher (WebTransport)
    let pub_session = Arc::new(connect_webtransport_client(addr, cert_der.clone()).await);
    publish_namespace(&pub_session, TrackNamespace::from(["example"].as_slice())).await;

    let _pub_keepalive = pub_session.clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                req.accept(1).await.unwrap();
                let mut group = pub_session.open_subgroup(1, 0, 0).await.unwrap();
                group.write_object(b"wt-to-quic").await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber (raw QUIC)
    let sub_session = Arc::new(connect_client(addr, cert_der).await);
    let _subscription = sub_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    match sub_session.next_event().await.unwrap() {
        SessionEvent::DataStream(mut group) => {
            let obj = group.read_object().await.unwrap().unwrap();
            assert_eq!(obj, b"wt-to-quic");
        }
        _ => panic!("expected DataStream event"),
    }

    pub_handle.await.unwrap();
}

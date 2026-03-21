use std::net::SocketAddr;
use std::sync::Once;

use moqt_core::data::object::ObjectHeader;

static INIT: Once = Once::new();

fn init_crypto() {
    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
    });
}
use moqt_core::data::subgroup_header::SubgroupHeader;
use moqt_core::message::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::message::publish_done::{PublishDoneMessage, STATUS_TRACK_ENDED};
use moqt_core::message::subscribe_ok::SubscribeOkMessage;
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::moqt_session::{MoqtSession, SessionEvent};
use moqt_core::quic_config;

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
    let client_config = quic_config::make_client_config(cert_der);
    let mut client_endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
    client_endpoint.set_default_client_config(client_config);

    let connection = client_endpoint
        .connect(relay_addr, "localhost")
        .unwrap()
        .await
        .unwrap();

    // SETUP exchange
    MoqtSession::connect(connection).await.unwrap()
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
            let _session = MoqtSession::accept(conn).await.unwrap();
        }
    });

    let _session = connect_client(addr, cert_der).await;
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

    let pub_session = connect_client(addr, cert_der).await;

    // Send PUBLISH_NAMESPACE
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;
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
    let pub_session = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;

    // Keep publisher connection alive for the duration of the test
    let _pub_conn = pub_session.connection().clone();

    // Publisher: spawn task to accept SUBSCRIBE and respond with SUBSCRIBE_OK
    tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                let ok = SubscribeOkMessage {
                    track_alias: 1,
                    parameters: vec![],
                    track_properties_raw: vec![],
                };
                req.accept(&ok).await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber connects and sends SUBSCRIBE
    let sub_session = connect_client(addr, cert_der).await;
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
    let pub_session = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;

    // Publisher: accept SUBSCRIBE, respond, then send objects
    // Keep a connection clone alive in outer scope so QUIC connection
    // survives after the spawned task completes.
    let _pub_conn_keepalive = pub_session.connection().clone();
    let pub_conn = pub_session.connection().clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                let ok = SubscribeOkMessage {
                    track_alias: 1,
                    parameters: vec![],
                    track_properties_raw: vec![],
                };
                req.accept(&ok).await.unwrap();

                // Send a subgroup with 2 objects
                let mut uni = pub_conn.open_uni().await.unwrap();
                let header = SubgroupHeader {
                    track_alias: 1,
                    group_id: 0,
                    has_properties: false,
                    end_of_group: true,
                    subgroup_id: None,
                    publisher_priority: None,
                };
                let mut data = Vec::new();
                header.encode(&mut data);

                let obj0 = ObjectHeader {
                    object_id_delta: 0,
                    payload_length: 5,
                };
                obj0.encode(&mut data);
                data.extend_from_slice(b"hello");

                let obj1 = ObjectHeader {
                    object_id_delta: 0,
                    payload_length: 5,
                };
                obj1.encode(&mut data);
                data.extend_from_slice(b"world");

                uni.write_all(&data).await.unwrap();
                uni.finish().unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber setup
    let sub_session = connect_client(addr, cert_der).await;
    let sub_conn = sub_session.connection().clone();
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
    let mut uni_recv = sub_conn.accept_uni().await.unwrap();
    let mut all_data = Vec::new();
    let mut tmp = vec![0u8; 4096];
    loop {
        match uni_recv.read(&mut tmp).await {
            Ok(Some(n)) => all_data.extend_from_slice(&tmp[..n]),
            Ok(None) => break,
            Err(e) => panic!("read error: {e}"),
        }
    }

    // Decode and verify
    let mut data_slice = all_data.as_slice();
    let header = SubgroupHeader::decode(&mut data_slice).unwrap();
    assert_eq!(header.track_alias, 1);
    assert_eq!(header.group_id, 0);

    let obj0 = ObjectHeader::decode(&mut data_slice, false).unwrap();
    assert_eq!(obj0.object_id_delta, 0);
    assert_eq!(obj0.payload_length, 5);
    let payload0 = &data_slice[..5];
    data_slice = &data_slice[5..];
    assert_eq!(payload0, b"hello");

    let obj1 = ObjectHeader::decode(&mut data_slice, false).unwrap();
    assert_eq!(obj1.object_id_delta, 0);
    assert_eq!(obj1.payload_length, 5);
    let payload1 = &data_slice[..5];
    assert_eq!(payload1, b"world");
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
    let pub_session = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;

    // Publisher: accept SUBSCRIBE, respond, send object, then PUBLISH_DONE
    let _pub_conn_keepalive = pub_session.connection().clone();
    let pub_conn = pub_session.connection().clone();
    tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                let ok = SubscribeOkMessage {
                    track_alias: 1,
                    parameters: vec![],
                    track_properties_raw: vec![],
                };
                req.accept(&ok).await.unwrap();

                // Send one object
                let mut uni = pub_conn.open_uni().await.unwrap();
                let header = SubgroupHeader {
                    track_alias: 1,
                    group_id: 0,
                    has_properties: false,
                    end_of_group: true,
                    subgroup_id: None,
                    publisher_priority: None,
                };
                let mut data = Vec::new();
                header.encode(&mut data);
                let obj = ObjectHeader {
                    object_id_delta: 0,
                    payload_length: 4,
                };
                obj.encode(&mut data);
                data.extend_from_slice(b"done");
                uni.write_all(&data).await.unwrap();
                uni.finish().unwrap();

                // Send PUBLISH_DONE on the bidi stream
                let done = PublishDoneMessage {
                    status_code: STATUS_TRACK_ENDED,
                    stream_count: 1,
                    reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase::from(""),
                };
                req.send_publish_done(&done).await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber setup
    let sub_session = connect_client(addr, cert_der).await;
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

    let pub_session = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;

    // Publisher: accept SUBSCRIBE, send 3 groups with 2 objects each
    let _pub_conn_keepalive = pub_session.connection().clone();
    let pub_conn = pub_session.connection().clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                let ok = SubscribeOkMessage {
                    track_alias: 1,
                    parameters: vec![],
                    track_properties_raw: vec![],
                };
                req.accept(&ok).await.unwrap();

                for group_id in 0u64..3 {
                    let mut uni = pub_conn.open_uni().await.unwrap();
                    let header = SubgroupHeader {
                        track_alias: 1,
                        group_id,
                        has_properties: false,
                        end_of_group: true,
                        subgroup_id: None,
                        publisher_priority: None,
                    };
                    let mut data = Vec::new();
                    header.encode(&mut data);

                    for obj_id in 0u64..2 {
                        let payload = format!("g{group_id}o{obj_id}");
                        let obj = ObjectHeader {
                            object_id_delta: 0,
                            payload_length: payload.len() as u64,
                        };
                        obj.encode(&mut data);
                        data.extend_from_slice(payload.as_bytes());
                    }
                    uni.write_all(&data).await.unwrap();
                    uni.finish().unwrap();
                }

                // PUBLISH_DONE
                let done = PublishDoneMessage {
                    status_code: STATUS_TRACK_ENDED,
                    stream_count: 3,
                    reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase::from(""),
                };
                req.send_publish_done(&done).await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber
    let sub_session = connect_client(addr, cert_der).await;
    let sub_conn = sub_session.connection().clone();
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
        let mut uni_recv = sub_conn.accept_uni().await.unwrap();
        let mut all_data = Vec::new();
        let mut tmp = vec![0u8; 4096];
        loop {
            match uni_recv.read(&mut tmp).await {
                Ok(Some(n)) => all_data.extend_from_slice(&tmp[..n]),
                Ok(None) => break,
                Err(e) => panic!("read error: {e}"),
            }
        }
        let mut data = all_data.as_slice();
        let header = SubgroupHeader::decode(&mut data).unwrap();
        let mut payloads = Vec::new();
        while !data.is_empty() {
            let obj = ObjectHeader::decode(&mut data, false).unwrap();
            let payload = std::str::from_utf8(&data[..obj.payload_length as usize])
                .unwrap()
                .to_string();
            data = &data[obj.payload_length as usize..];
            payloads.push(payload);
        }
        received_groups.push((header.group_id, payloads));
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
    let pub_session = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;

    // Publisher: accept SUBSCRIBE (will come later), respond, send objects
    let _pub_conn_keepalive = pub_session.connection().clone();
    let pub_conn = pub_session.connection().clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                let ok = SubscribeOkMessage {
                    track_alias: 1,
                    parameters: vec![],
                    track_properties_raw: vec![],
                };
                req.accept(&ok).await.unwrap();

                // Send 2 groups after subscriber joins
                for group_id in 0u64..2 {
                    let mut uni = pub_conn.open_uni().await.unwrap();
                    let header = SubgroupHeader {
                        track_alias: 1,
                        group_id,
                        has_properties: false,
                        end_of_group: true,
                        subgroup_id: None,
                        publisher_priority: None,
                    };
                    let mut data = Vec::new();
                    header.encode(&mut data);

                    let payload = format!("late-g{group_id}");
                    let obj = ObjectHeader {
                        object_id_delta: 0,
                        payload_length: payload.len() as u64,
                    };
                    obj.encode(&mut data);
                    data.extend_from_slice(payload.as_bytes());

                    uni.write_all(&data).await.unwrap();
                    uni.finish().unwrap();
                }

                let done = PublishDoneMessage {
                    status_code: STATUS_TRACK_ENDED,
                    stream_count: 2,
                    reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase::from(""),
                };
                req.send_publish_done(&done).await.unwrap();
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber connects AFTER publisher is ready (simulating late join)
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let sub_session = connect_client(addr, cert_der).await;
    let sub_conn = sub_session.connection().clone();
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
    let mut uni_recv = sub_conn.accept_uni().await.unwrap();
    let mut all_data = Vec::new();
    let mut tmp = vec![0u8; 4096];
    loop {
        match uni_recv.read(&mut tmp).await {
            Ok(Some(n)) => all_data.extend_from_slice(&tmp[..n]),
            Ok(None) => break,
            Err(e) => panic!("read error: {e}"),
        }
    }
    let mut data = all_data.as_slice();
    let header = SubgroupHeader::decode(&mut data).unwrap();
    // Should receive at least the first group
    assert_eq!(header.track_alias, 1);

    let obj = ObjectHeader::decode(&mut data, false).unwrap();
    let payload = std::str::from_utf8(&data[..obj.payload_length as usize]).unwrap();
    assert!(payload.starts_with("late-g"));
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
    let sub_session = connect_client(addr, cert_der).await;
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
    let pub_session = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;

    // Publisher: accept 2 SUBSCRIBE (one per subscriber), respond to each, then send objects
    let _pub_conn_keepalive = pub_session.connection().clone();
    let pub_conn = pub_session.connection().clone();
    let pub_handle = tokio::spawn(async move {
        // Accept first SUBSCRIBE
        let event1 = pub_session.next_event().await.unwrap();
        let mut req1 = match event1 {
            SessionEvent::Subscribe(req) => req,
            _ => panic!("expected Subscribe event"),
        };
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        req1.accept(&ok).await.unwrap();

        // Accept second SUBSCRIBE
        let event2 = pub_session.next_event().await.unwrap();
        let mut req2 = match event2 {
            SessionEvent::Subscribe(req) => req,
            _ => panic!("expected Subscribe event"),
        };
        req2.accept(&ok).await.unwrap();

        // Small delay so both subscriptions are registered before sending
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Send 1 group with 1 object
        let mut uni = pub_conn.open_uni().await.unwrap();
        let header = SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
            end_of_group: true,
            subgroup_id: None,
            publisher_priority: None,
        };
        let mut data = Vec::new();
        header.encode(&mut data);
        let obj = ObjectHeader {
            object_id_delta: 0,
            payload_length: 6,
        };
        obj.encode(&mut data);
        data.extend_from_slice(b"shared");
        uni.write_all(&data).await.unwrap();
        uni.finish().unwrap();

        // PUBLISH_DONE on both bidi streams
        let done = PublishDoneMessage {
            status_code: STATUS_TRACK_ENDED,
            stream_count: 1,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase::from(""),
        };
        req1.send_publish_done(&done).await.unwrap();
        req2.send_publish_done(&done).await.unwrap();
    });

    // Helper to subscribe and receive objects
    async fn subscribe_and_receive(
        addr: SocketAddr,
        cert_der: rustls_pki_types::CertificateDer<'static>,
    ) -> Vec<u8> {
        let session = connect_client(addr, cert_der).await;
        let conn = session.connection().clone();
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

        let mut uni_recv = conn.accept_uni().await.unwrap();
        let mut all_data = Vec::new();
        let mut tmp = vec![0u8; 4096];
        loop {
            match uni_recv.read(&mut tmp).await {
                Ok(Some(n)) => all_data.extend_from_slice(&tmp[..n]),
                Ok(None) => break,
                Err(e) => panic!("read error: {e}"),
            }
        }
        // Extract payload
        let mut data = all_data.as_slice();
        let _header = SubgroupHeader::decode(&mut data).unwrap();
        let obj = ObjectHeader::decode(&mut data, false).unwrap();
        data[..obj.payload_length as usize].to_vec()
    }

    // Two subscribers connect concurrently
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
    let pub_session = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;

    // Publisher: accept 2 SUBSCRIBEs (video + audio), send objects on each
    let _pub_conn_keepalive = pub_session.connection().clone();
    let pub_conn = pub_session.connection().clone();
    let pub_handle = tokio::spawn(async move {
        // Accept SUBSCRIBE for video (alias=1)
        let event_v = pub_session.next_event().await.unwrap();
        let mut req_v = match event_v {
            SessionEvent::Subscribe(req) => req,
            _ => panic!("expected Subscribe event"),
        };
        let ok_v = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        req_v.accept(&ok_v).await.unwrap();

        // Accept SUBSCRIBE for audio (alias=2)
        let event_a = pub_session.next_event().await.unwrap();
        let mut req_a = match event_a {
            SessionEvent::Subscribe(req) => req,
            _ => panic!("expected Subscribe event"),
        };
        let ok_a = SubscribeOkMessage {
            track_alias: 2,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        req_a.accept(&ok_a).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send video object
        let mut uni_v = pub_conn.open_uni().await.unwrap();
        let mut data_v = Vec::new();
        SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
            end_of_group: true,
            subgroup_id: None,
            publisher_priority: None,
        }
        .encode(&mut data_v);
        ObjectHeader {
            object_id_delta: 0,
            payload_length: 5,
        }
        .encode(&mut data_v);
        data_v.extend_from_slice(b"video");
        uni_v.write_all(&data_v).await.unwrap();
        uni_v.finish().unwrap();

        // Send audio object
        let mut uni_a = pub_conn.open_uni().await.unwrap();
        let mut data_a = Vec::new();
        SubgroupHeader {
            track_alias: 2,
            group_id: 0,
            has_properties: false,
            end_of_group: true,
            subgroup_id: None,
            publisher_priority: None,
        }
        .encode(&mut data_a);
        ObjectHeader {
            object_id_delta: 0,
            payload_length: 5,
        }
        .encode(&mut data_a);
        data_a.extend_from_slice(b"audio");
        uni_a.write_all(&data_a).await.unwrap();
        uni_a.finish().unwrap();

        // PUBLISH_DONE on both
        let done = PublishDoneMessage {
            status_code: STATUS_TRACK_ENDED,
            stream_count: 1,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase::from(""),
        };
        req_v.send_publish_done(&done).await.unwrap();
        req_a.send_publish_done(&done).await.unwrap();
    });

    // Subscriber: subscribe to both tracks
    let sub_session = connect_client(addr, cert_der).await;
    let sub_conn = sub_session.connection().clone();

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
        let mut uni_recv = sub_conn.accept_uni().await.unwrap();
        let mut all_data = Vec::new();
        let mut tmp = vec![0u8; 4096];
        loop {
            match uni_recv.read(&mut tmp).await {
                Ok(Some(n)) => all_data.extend_from_slice(&tmp[..n]),
                Ok(None) => break,
                Err(e) => panic!("read error: {e}"),
            }
        }
        let mut data = all_data.as_slice();
        let _header = SubgroupHeader::decode(&mut data).unwrap();
        let obj = ObjectHeader::decode(&mut data, false).unwrap();
        let payload = std::str::from_utf8(&data[..obj.payload_length as usize]).unwrap();
        payloads.push(payload.to_string());
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
    let pub_session = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;

    // Publisher: accept exactly 1 SUBSCRIBE, send data, then PUBLISH_DONE
    let pub_conn = pub_session.connection().clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        let mut req = match event {
            SessionEvent::Subscribe(req) => req,
            _ => panic!("expected Subscribe event"),
        };
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        req.accept(&ok).await.unwrap();

        // Wait for both subscribers to be registered
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Send 1 group with 1 object
        let mut uni = pub_conn.open_uni().await.unwrap();
        let mut data = Vec::new();
        SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
            end_of_group: true,
            subgroup_id: None,
            publisher_priority: None,
        }
        .encode(&mut data);
        ObjectHeader {
            object_id_delta: 0,
            payload_length: 4,
        }
        .encode(&mut data);
        data.extend_from_slice(b"aggr");
        uni.write_all(&data).await.unwrap();
        uni.finish().unwrap();

        let done = PublishDoneMessage {
            status_code: STATUS_TRACK_ENDED,
            stream_count: 1,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase::from(""),
        };
        req.send_publish_done(&done).await.unwrap();

        // Verify no second SUBSCRIBE arrives (would timeout)
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            pub_session.next_event(),
        )
        .await;
        assert!(result.is_err(), "publisher should NOT receive a second SUBSCRIBE");
    });

    // Subscriber 1: subscribe and wait for SUBSCRIBE_OK
    let sub1_session = connect_client(addr, cert_der.clone()).await;
    let sub1_conn = sub1_session.connection().clone();
    let _sub1 = sub1_session
        .subscribe(
            TrackNamespace::from(["example"].as_slice()),
            "video",
            vec![],
        )
        .await
        .unwrap();

    // Subscriber 2: subscribe AFTER sub1 is established (triggers aggregation)
    let sub2_session = connect_client(addr, cert_der).await;
    let sub2_conn = sub2_session.connection().clone();
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
        let mut uni_recv = sub1_conn.accept_uni().await.unwrap();
        let mut all_data = Vec::new();
        let mut tmp = vec![0u8; 4096];
        loop {
            match uni_recv.read(&mut tmp).await {
                Ok(Some(n)) => all_data.extend_from_slice(&tmp[..n]),
                Ok(None) => break,
                Err(e) => panic!("read error: {e}"),
            }
        }
        let mut slice = all_data.as_slice();
        let _header = SubgroupHeader::decode(&mut slice).unwrap();
        let obj = ObjectHeader::decode(&mut slice, false).unwrap();
        slice[..obj.payload_length as usize].to_vec()
    });

    let recv2 = tokio::spawn(async move {
        let mut uni_recv = sub2_conn.accept_uni().await.unwrap();
        let mut all_data = Vec::new();
        let mut tmp = vec![0u8; 4096];
        loop {
            match uni_recv.read(&mut tmp).await {
                Ok(Some(n)) => all_data.extend_from_slice(&tmp[..n]),
                Ok(None) => break,
                Err(e) => panic!("read error: {e}"),
            }
        }
        let mut slice = all_data.as_slice();
        let _header = SubgroupHeader::decode(&mut slice).unwrap();
        let obj = ObjectHeader::decode(&mut slice, false).unwrap();
        slice[..obj.payload_length as usize].to_vec()
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

    let pub_session = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_session,
        TrackNamespace::from(["example"].as_slice()),
    )
    .await;

    // Publisher: accept SUBSCRIBE, respond, send objects continuously
    let _pub_conn_keepalive = pub_session.connection().clone();
    let pub_conn = pub_session.connection().clone();
    let pub_handle = tokio::spawn(async move {
        let event = pub_session.next_event().await.unwrap();
        match event {
            SessionEvent::Subscribe(mut req) => {
                let ok = SubscribeOkMessage {
                    track_alias: 1,
                    parameters: vec![],
                    track_properties_raw: vec![],
                };
                req.accept(&ok).await.unwrap();

                // Send a few groups
                for group_id in 0u64..3 {
                    let mut uni = pub_conn.open_uni().await.unwrap();
                    let mut data = Vec::new();
                    SubgroupHeader {
                        track_alias: 1,
                        group_id,
                        has_properties: false,
                        end_of_group: true,
                        subgroup_id: None,
                        publisher_priority: None,
                    }
                    .encode(&mut data);
                    ObjectHeader {
                        object_id_delta: 0,
                        payload_length: 4,
                    }
                    .encode(&mut data);
                    data.extend_from_slice(b"data");
                    uni.write_all(&data).await.unwrap();
                    uni.finish().unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }

                // Publisher connection should still be alive
                assert!(
                    pub_conn.close_reason().is_none(),
                    "publisher should still be connected"
                );
            }
            _ => panic!("expected Subscribe event"),
        }
    });

    // Subscriber connects, subscribes, receives 1 group, then disconnects
    {
        let sub_session = connect_client(addr, cert_der).await;
        let sub_conn = sub_session.connection().clone();
        let _subscription = sub_session
            .subscribe(
                TrackNamespace::from(["example"].as_slice()),
                "video",
                vec![],
            )
            .await
            .unwrap();

        // Receive at least 1 object
        let mut uni_recv = sub_conn.accept_uni().await.unwrap();
        let mut tmp = vec![0u8; 4096];
        let _ = uni_recv.read(&mut tmp).await.unwrap();

        // Disconnect subscriber
        sub_conn.close(0u32.into(), b"done");
    }

    // Publisher should complete without error
    pub_handle.await.unwrap();
}

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
use moqt_core::message::publish_done::PublishDoneMessage;
use moqt_core::message::publish_namespace::PublishNamespaceMessage;
use moqt_core::message::request_error::RequestErrorMessage;
use moqt_core::message::request_ok::RequestOkMessage;
use moqt_core::message::setup::{SetupMessage, SetupOption};
use moqt_core::message::subscribe::SubscribeMessage;
use moqt_core::message::subscribe_ok::SubscribeOkMessage;
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::control_stream::ControlStreamReader;
use moqt_core::session::quic_config;

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
) -> quinn::Connection {
    let client_config = quic_config::make_client_config(cert_der);
    let mut client_endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
    client_endpoint.set_default_client_config(client_config);

    let connection = client_endpoint
        .connect(relay_addr, "localhost")
        .unwrap()
        .await
        .unwrap();

    // Send SETUP on our control stream
    let mut ctrl_send = connection.open_uni().await.unwrap();
    let setup = SetupMessage {
        setup_options: vec![
            SetupOption::Path(b"/".to_vec()),
            SetupOption::Authority(b"localhost".to_vec()),
        ],
    };
    let mut buf = Vec::new();
    setup.encode(&mut buf).unwrap();
    ctrl_send.write_all(&buf).await.unwrap();

    // Accept relay's control stream and read SETUP
    let recv = connection.accept_uni().await.unwrap();
    let mut reader = ControlStreamReader::new(recv);
    let _relay_setup = reader.read_setup().await.unwrap();

    connection
}

/// Helper: send PUBLISH_NAMESPACE and receive REQUEST_OK
async fn publish_namespace(conn: &quinn::Connection, namespace: TrackNamespace) {
    let (mut send, recv) = conn.open_bi().await.unwrap();
    let msg = PublishNamespaceMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: namespace,
    };
    let mut buf = Vec::new();
    msg.encode(&mut buf).unwrap();
    send.write_all(&buf).await.unwrap();

    let mut reader = ControlStreamReader::new(recv);
    let ok_bytes = reader.read_message_bytes().await.unwrap();
    let mut slice = ok_bytes.as_slice();
    let _ok = RequestOkMessage::decode(&mut slice).unwrap();
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
            // Send SETUP
            let mut ctrl = conn.open_uni().await.unwrap();
            let setup = SetupMessage {
                setup_options: vec![],
            };
            let mut buf = Vec::new();
            setup.encode(&mut buf).unwrap();
            ctrl.write_all(&buf).await.unwrap();
            // Accept peer's SETUP
            let recv = conn.accept_uni().await.unwrap();
            let mut reader = ControlStreamReader::new(recv);
            let _setup = reader.read_setup().await.unwrap();
        }
    });

    let _conn = connect_client(addr, cert_der).await;
    // If we get here, SETUP exchange succeeded
}

/// 4.1: PUBLISH_NAMESPACE → REQUEST_OK
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

    let conn = connect_client(addr, cert_der).await;

    // Send PUBLISH_NAMESPACE
    publish_namespace(
        &conn,
        TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
    )
    .await;
    // If we get here, registration succeeded
}

/// 4.2: SUBSCRIBE → SUBSCRIBE_OK (via Relay)
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
    let pub_conn = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_conn,
        TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
    )
    .await;

    // Publisher: spawn task to accept SUBSCRIBE and respond with SUBSCRIBE_OK
    let pub_conn2 = pub_conn.clone();
    tokio::spawn(async move {
        let (mut send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = ControlStreamReader::new(recv);
        let sub_bytes = reader.read_message_bytes().await.unwrap();
        let mut slice = sub_bytes.as_slice();
        let _subscribe = SubscribeMessage::decode(&mut slice).unwrap();

        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        let mut buf = Vec::new();
        ok.encode(&mut buf).unwrap();
        send.write_all(&buf).await.unwrap();
    });

    // Subscriber connects and sends SUBSCRIBE
    let sub_conn = connect_client(addr, cert_der).await;
    let (mut sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
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
    let mut buf = Vec::new();
    subscribe.encode(&mut buf).unwrap();
    sub_send.write_all(&buf).await.unwrap();

    // Read SUBSCRIBE_OK
    let mut reader = ControlStreamReader::new(sub_recv);
    let ok_bytes = reader.read_message_bytes().await.unwrap();
    let mut slice = ok_bytes.as_slice();
    let subscribe_ok = SubscribeOkMessage::decode(&mut slice).unwrap();
    assert_eq!(subscribe_ok.track_alias, 1);
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
    let pub_conn = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_conn,
        TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
    )
    .await;

    // Publisher: accept SUBSCRIBE, respond, then send objects
    let pub_conn2 = pub_conn.clone();
    let pub_handle = tokio::spawn(async move {
        let (mut send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = ControlStreamReader::new(recv);
        let sub_bytes = reader.read_message_bytes().await.unwrap();
        let mut slice = sub_bytes.as_slice();
        let _subscribe = SubscribeMessage::decode(&mut slice).unwrap();

        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        let mut buf = Vec::new();
        ok.encode(&mut buf).unwrap();
        send.write_all(&buf).await.unwrap();

        // Send a subgroup with 2 objects
        let mut uni = pub_conn2.open_uni().await.unwrap();
        let header = SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
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
    });

    // Subscriber setup
    let sub_conn = connect_client(addr, cert_der).await;
    let (mut sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    let mut buf = Vec::new();
    subscribe.encode(&mut buf).unwrap();
    sub_send.write_all(&buf).await.unwrap();

    // Read SUBSCRIBE_OK
    let mut reader = ControlStreamReader::new(sub_recv);
    let ok_bytes = reader.read_message_bytes().await.unwrap();
    let mut slice = ok_bytes.as_slice();
    let _subscribe_ok = SubscribeOkMessage::decode(&mut slice).unwrap();

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
    let pub_conn = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_conn,
        TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
    )
    .await;

    // Publisher: accept SUBSCRIBE, respond, send object, then PUBLISH_DONE
    let pub_conn2 = pub_conn.clone();
    tokio::spawn(async move {
        let (mut send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = ControlStreamReader::new(recv);
        let sub_bytes = reader.read_message_bytes().await.unwrap();
        let mut slice = sub_bytes.as_slice();
        let _subscribe = SubscribeMessage::decode(&mut slice).unwrap();

        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        let mut buf = Vec::new();
        ok.encode(&mut buf).unwrap();
        send.write_all(&buf).await.unwrap();

        // Send one object
        let mut uni = pub_conn2.open_uni().await.unwrap();
        let header = SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
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
            status_code: 0x2, // TRACK_ENDED
            stream_count: 1,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        let mut done_buf = Vec::new();
        done.encode(&mut done_buf);
        send.write_all(&done_buf).await.unwrap();
    });

    // Subscriber setup
    let sub_conn = connect_client(addr, cert_der).await;
    let (mut sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    let mut buf = Vec::new();
    subscribe.encode(&mut buf).unwrap();
    sub_send.write_all(&buf).await.unwrap();

    // Read SUBSCRIBE_OK
    let mut sub_reader = ControlStreamReader::new(sub_recv);
    let ok_bytes = sub_reader.read_message_bytes().await.unwrap();
    let mut slice = ok_bytes.as_slice();
    let _subscribe_ok = SubscribeOkMessage::decode(&mut slice).unwrap();

    // Read PUBLISH_DONE (forwarded by relay)
    let done_bytes = sub_reader.read_message_bytes().await.unwrap();
    let mut done_slice = done_bytes.as_slice();
    let publish_done = PublishDoneMessage::decode(&mut done_slice).unwrap();
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

    let pub_conn = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_conn,
        TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
    )
    .await;

    // Publisher: accept SUBSCRIBE, send 3 groups with 2 objects each
    let pub_conn2 = pub_conn.clone();
    let pub_handle = tokio::spawn(async move {
        let (mut send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = ControlStreamReader::new(recv);
        let sub_bytes = reader.read_message_bytes().await.unwrap();
        let mut slice = sub_bytes.as_slice();
        let _subscribe = SubscribeMessage::decode(&mut slice).unwrap();

        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        let mut buf = Vec::new();
        ok.encode(&mut buf).unwrap();
        send.write_all(&buf).await.unwrap();

        for group_id in 0u64..3 {
            let mut uni = pub_conn2.open_uni().await.unwrap();
            let header = SubgroupHeader {
                track_alias: 1,
                group_id,
                has_properties: false,
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
            status_code: 0x2,
            stream_count: 3,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        buf.clear();
        done.encode(&mut buf);
        send.write_all(&buf).await.unwrap();
    });

    // Subscriber
    let sub_conn = connect_client(addr, cert_der).await;
    let (mut sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    let mut buf = Vec::new();
    subscribe.encode(&mut buf).unwrap();
    sub_send.write_all(&buf).await.unwrap();

    let mut sub_reader = ControlStreamReader::new(sub_recv);
    let ok_bytes = sub_reader.read_message_bytes().await.unwrap();
    let mut slice = ok_bytes.as_slice();
    let _subscribe_ok = SubscribeOkMessage::decode(&mut slice).unwrap();

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
    let done_bytes = sub_reader.read_message_bytes().await.unwrap();
    let mut done_slice = done_bytes.as_slice();
    let publish_done = PublishDoneMessage::decode(&mut done_slice).unwrap();
    assert_eq!(publish_done.stream_count, 3);
}

/// 7.2: Late join — Subscriber connects while Publisher is already sending
#[tokio::test]
async fn late_join() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher connects first and registers
    let pub_conn = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_conn,
        TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
    )
    .await;

    // Publisher: accept SUBSCRIBE (will come later), respond, send objects
    let pub_conn2 = pub_conn.clone();
    let pub_handle = tokio::spawn(async move {
        let (mut send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = ControlStreamReader::new(recv);
        let sub_bytes = reader.read_message_bytes().await.unwrap();
        let mut slice = sub_bytes.as_slice();
        let _subscribe = SubscribeMessage::decode(&mut slice).unwrap();

        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        let mut buf = Vec::new();
        ok.encode(&mut buf).unwrap();
        send.write_all(&buf).await.unwrap();

        // Send 2 groups after subscriber joins
        for group_id in 0u64..2 {
            let mut uni = pub_conn2.open_uni().await.unwrap();
            let header = SubgroupHeader {
                track_alias: 1,
                group_id,
                has_properties: false,
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
            status_code: 0x2,
            stream_count: 2,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        buf.clear();
        done.encode(&mut buf);
        send.write_all(&buf).await.unwrap();
    });

    // Subscriber connects AFTER publisher is ready (simulating late join)
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let sub_conn = connect_client(addr, cert_der).await;
    let (mut sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
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
    let mut buf = Vec::new();
    subscribe.encode(&mut buf).unwrap();
    sub_send.write_all(&buf).await.unwrap();

    let mut sub_reader = ControlStreamReader::new(sub_recv);
    let ok_bytes = sub_reader.read_message_bytes().await.unwrap();
    let mut slice = ok_bytes.as_slice();
    let _subscribe_ok = SubscribeOkMessage::decode(&mut slice).unwrap();

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

/// 3.1: ALPN mismatch — connection should fail
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

/// 4.3: SUBSCRIBE to unknown namespace → REQUEST_ERROR
#[tokio::test]
async fn subscribe_unknown_namespace() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Subscriber connects (no publisher registered)
    let sub_conn = connect_client(addr, cert_der).await;
    let (mut sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"nonexistent".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    let mut buf = Vec::new();
    subscribe.encode(&mut buf).unwrap();
    sub_send.write_all(&buf).await.unwrap();

    // Should receive REQUEST_ERROR
    let mut reader = ControlStreamReader::new(sub_recv);
    let err_bytes = reader.read_message_bytes().await.unwrap();
    let mut slice = err_bytes.as_slice();
    let err = RequestErrorMessage::decode(&mut slice).unwrap();
    assert_eq!(err.error_code, 0x10); // DOES_NOT_EXIST
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
    let pub_conn = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_conn,
        TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
    )
    .await;

    // Publisher: accept 2 SUBSCRIBE (one per subscriber), respond to each, then send objects
    let pub_conn2 = pub_conn.clone();
    let pub_handle = tokio::spawn(async move {
        // Accept first SUBSCRIBE
        let (mut send1, recv1) = pub_conn2.accept_bi().await.unwrap();
        let mut reader1 = ControlStreamReader::new(recv1);
        let sub_bytes1 = reader1.read_message_bytes().await.unwrap();
        let mut s1 = sub_bytes1.as_slice();
        let _sub1 = SubscribeMessage::decode(&mut s1).unwrap();
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        let mut buf = Vec::new();
        ok.encode(&mut buf).unwrap();
        send1.write_all(&buf).await.unwrap();

        // Accept second SUBSCRIBE
        let (mut send2, recv2) = pub_conn2.accept_bi().await.unwrap();
        let mut reader2 = ControlStreamReader::new(recv2);
        let sub_bytes2 = reader2.read_message_bytes().await.unwrap();
        let mut s2 = sub_bytes2.as_slice();
        let _sub2 = SubscribeMessage::decode(&mut s2).unwrap();
        let mut buf2 = Vec::new();
        ok.encode(&mut buf2).unwrap();
        send2.write_all(&buf2).await.unwrap();

        // Small delay so both subscriptions are registered before sending
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Send 1 group with 1 object
        let mut uni = pub_conn2.open_uni().await.unwrap();
        let header = SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
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
            status_code: 0x2,
            stream_count: 1,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        let mut done_buf = Vec::new();
        done.encode(&mut done_buf);
        send1.write_all(&done_buf).await.unwrap();
        send2.write_all(&done_buf).await.unwrap();
    });

    // Helper to subscribe and receive objects
    async fn subscribe_and_receive(
        addr: SocketAddr,
        cert_der: rustls_pki_types::CertificateDer<'static>,
    ) -> Vec<u8> {
        let conn = connect_client(addr, cert_der).await;
        let (mut sub_send, sub_recv) = conn.open_bi().await.unwrap();
        let subscribe = SubscribeMessage {
            request_id: 0,
            required_request_id_delta: 0,
            track_namespace: TrackNamespace {
                fields: vec![b"example".to_vec()],
            },
            track_name: b"video".to_vec(),
            parameters: vec![],
        };
        let mut buf = Vec::new();
        subscribe.encode(&mut buf).unwrap();
        sub_send.write_all(&buf).await.unwrap();

        let mut reader = ControlStreamReader::new(sub_recv);
        let _ok_bytes = reader.read_message_bytes().await.unwrap();

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

/// 5.4: Multiple tracks — video and audio simultaneously
#[tokio::test]
async fn multiple_tracks() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publisher
    let pub_conn = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_conn,
        TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
    )
    .await;

    // Publisher: accept 2 SUBSCRIBEs (video + audio), send objects on each
    let pub_conn2 = pub_conn.clone();
    let pub_handle = tokio::spawn(async move {
        // Accept SUBSCRIBE for video (alias=1)
        let (mut send_v, recv_v) = pub_conn2.accept_bi().await.unwrap();
        let mut reader_v = ControlStreamReader::new(recv_v);
        let _ = reader_v.read_message_bytes().await.unwrap();
        let ok_v = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        let mut buf = Vec::new();
        ok_v.encode(&mut buf).unwrap();
        send_v.write_all(&buf).await.unwrap();

        // Accept SUBSCRIBE for audio (alias=2)
        let (mut send_a, recv_a) = pub_conn2.accept_bi().await.unwrap();
        let mut reader_a = ControlStreamReader::new(recv_a);
        let _ = reader_a.read_message_bytes().await.unwrap();
        let ok_a = SubscribeOkMessage {
            track_alias: 2,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        buf.clear();
        ok_a.encode(&mut buf).unwrap();
        send_a.write_all(&buf).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send video object
        let mut uni_v = pub_conn2.open_uni().await.unwrap();
        let mut data_v = Vec::new();
        SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
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
        let mut uni_a = pub_conn2.open_uni().await.unwrap();
        let mut data_a = Vec::new();
        SubgroupHeader {
            track_alias: 2,
            group_id: 0,
            has_properties: false,
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
            status_code: 0x2,
            stream_count: 1,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        buf.clear();
        done.encode(&mut buf);
        send_v.write_all(&buf).await.unwrap();
        send_a.write_all(&buf).await.unwrap();
    });

    // Subscriber: subscribe to both tracks
    let sub_conn = connect_client(addr, cert_der).await;

    // Subscribe to video
    let (mut sub_send_v, sub_recv_v) = sub_conn.open_bi().await.unwrap();
    let sub_v = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    let mut buf = Vec::new();
    sub_v.encode(&mut buf).unwrap();
    sub_send_v.write_all(&buf).await.unwrap();
    let mut reader_v = ControlStreamReader::new(sub_recv_v);
    let _ = reader_v.read_message_bytes().await.unwrap(); // SUBSCRIBE_OK

    // Subscribe to audio
    let (mut sub_send_a, sub_recv_a) = sub_conn.open_bi().await.unwrap();
    let sub_a = SubscribeMessage {
        request_id: 2,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"audio".to_vec(),
        parameters: vec![],
    };
    buf.clear();
    sub_a.encode(&mut buf).unwrap();
    sub_send_a.write_all(&buf).await.unwrap();
    let mut reader_a = ControlStreamReader::new(sub_recv_a);
    let _ = reader_a.read_message_bytes().await.unwrap(); // SUBSCRIBE_OK

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

/// 6.3: Subscriber disconnect — relay cleans up, publisher continues
#[tokio::test]
async fn subscriber_disconnect() {
    init_crypto();
    let (endpoint, addr, cert_der) = start_relay().await;

    let relay = moqt_relay::relay::Relay::new(endpoint);
    tokio::spawn(async move { relay.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let pub_conn = connect_client(addr, cert_der.clone()).await;
    publish_namespace(
        &pub_conn,
        TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
    )
    .await;

    // Publisher: accept SUBSCRIBE, respond, send objects continuously
    let pub_conn2 = pub_conn.clone();
    let pub_handle = tokio::spawn(async move {
        let (mut send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = ControlStreamReader::new(recv);
        let _ = reader.read_message_bytes().await.unwrap();
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        let mut buf = Vec::new();
        ok.encode(&mut buf).unwrap();
        send.write_all(&buf).await.unwrap();

        // Send a few groups
        for group_id in 0u64..3 {
            let mut uni = pub_conn2.open_uni().await.unwrap();
            let mut data = Vec::new();
            SubgroupHeader {
                track_alias: 1,
                group_id,
                has_properties: false,
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
            pub_conn2.close_reason().is_none(),
            "publisher should still be connected"
        );
    });

    // Subscriber connects, subscribes, receives 1 group, then disconnects
    {
        let sub_conn = connect_client(addr, cert_der).await;
        let (mut sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
        let subscribe = SubscribeMessage {
            request_id: 0,
            required_request_id_delta: 0,
            track_namespace: TrackNamespace {
                fields: vec![b"example".to_vec()],
            },
            track_name: b"video".to_vec(),
            parameters: vec![],
        };
        let mut buf = Vec::new();
        subscribe.encode(&mut buf).unwrap();
        sub_send.write_all(&buf).await.unwrap();

        let mut reader = ControlStreamReader::new(sub_recv);
        let _ = reader.read_message_bytes().await.unwrap(); // SUBSCRIBE_OK

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

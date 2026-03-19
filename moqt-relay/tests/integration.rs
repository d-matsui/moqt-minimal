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
use moqt_core::message::setup::{SetupMessage, SetupOption};
use moqt_core::message::subscribe::SubscribeMessage;
use moqt_core::message::subscribe_ok::SubscribeOkMessage;
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::control_stream::{ControlStreamReader, ControlStreamWriter};
use moqt_core::session::quic_config;
use moqt_core::session::request_stream::{
    RequestMessage, RequestStreamReader, RequestStreamWriter,
};

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
    let ctrl_send = connection.open_uni().await.unwrap();
    let mut ctrl_writer = ControlStreamWriter::new(ctrl_send);
    let setup = SetupMessage {
        setup_options: vec![
            SetupOption::Path(b"/".to_vec()),
            SetupOption::Authority(b"localhost".to_vec()),
        ],
    };
    ctrl_writer.write_setup(&setup).await.unwrap();

    // Accept relay's control stream and read SETUP
    let recv = connection.accept_uni().await.unwrap();
    let mut reader = ControlStreamReader::new(recv);
    let _relay_setup = reader.read_setup().await.unwrap();

    connection
}

/// Helper: send PUBLISH_NAMESPACE and receive REQUEST_OK
async fn publish_namespace(conn: &quinn::Connection, namespace: TrackNamespace) {
    let (send, recv) = conn.open_bi().await.unwrap();
    let mut writer = RequestStreamWriter::new(send);
    let msg = PublishNamespaceMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: namespace,
    };
    writer.write_publish_namespace(&msg).await.unwrap();

    let mut reader = RequestStreamReader::new(recv);
    let response = reader.read_message().await.unwrap();
    assert!(matches!(response, RequestMessage::RequestOk(_)));
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
            let ctrl = conn.open_uni().await.unwrap();
            let mut ctrl_writer = ControlStreamWriter::new(ctrl);
            let setup = SetupMessage {
                setup_options: vec![],
            };
            ctrl_writer.write_setup(&setup).await.unwrap();
            // Accept peer's SETUP
            let recv = conn.accept_uni().await.unwrap();
            let mut reader = ControlStreamReader::new(recv);
            let _setup = reader.read_setup().await.unwrap();
        }
    });

    let _conn = connect_client(addr, cert_der).await;
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
        let (send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = RequestStreamReader::new(recv);
        let msg = reader.read_message().await.unwrap();
        assert!(matches!(msg, RequestMessage::Subscribe(_)));

        let mut writer = RequestStreamWriter::new(send);
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        writer.write_subscribe_ok(&ok).await.unwrap();
    });

    // Subscriber connects and sends SUBSCRIBE
    let sub_conn = connect_client(addr, cert_der).await;
    let (sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let mut sub_writer = RequestStreamWriter::new(sub_send);
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
    sub_writer.write_subscribe(&subscribe).await.unwrap();

    // Read SUBSCRIBE_OK
    let mut reader = RequestStreamReader::new(sub_recv);
    let msg = reader.read_message().await.unwrap();
    match msg {
        RequestMessage::SubscribeOk(subscribe_ok) => {
            assert_eq!(subscribe_ok.track_alias, 1);
        }
        other => panic!("expected SubscribeOk, got {:?}", msg_name(&other)),
    }
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
        let (send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = RequestStreamReader::new(recv);
        let msg = reader.read_message().await.unwrap();
        assert!(matches!(msg, RequestMessage::Subscribe(_)));

        let mut writer = RequestStreamWriter::new(send);
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        writer.write_subscribe_ok(&ok).await.unwrap();

        // Send a subgroup with 2 objects
        let mut uni = pub_conn2.open_uni().await.unwrap();
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
    });

    // Subscriber setup
    let sub_conn = connect_client(addr, cert_der).await;
    let (sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let mut sub_writer = RequestStreamWriter::new(sub_send);
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    sub_writer.write_subscribe(&subscribe).await.unwrap();

    // Read SUBSCRIBE_OK
    let mut reader = RequestStreamReader::new(sub_recv);
    let msg = reader.read_message().await.unwrap();
    assert!(matches!(msg, RequestMessage::SubscribeOk(_)));

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
        let (send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = RequestStreamReader::new(recv);
        let msg = reader.read_message().await.unwrap();
        assert!(matches!(msg, RequestMessage::Subscribe(_)));

        let mut writer = RequestStreamWriter::new(send);
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        writer.write_subscribe_ok(&ok).await.unwrap();

        // Send one object
        let mut uni = pub_conn2.open_uni().await.unwrap();
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
            status_code: 0x2, // TRACK_ENDED
            stream_count: 1,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        writer.write_publish_done(&done).await.unwrap();
    });

    // Subscriber setup
    let sub_conn = connect_client(addr, cert_der).await;
    let (sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let mut sub_writer = RequestStreamWriter::new(sub_send);
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    sub_writer.write_subscribe(&subscribe).await.unwrap();

    // Read SUBSCRIBE_OK
    let mut sub_reader = RequestStreamReader::new(sub_recv);
    let msg = sub_reader.read_message().await.unwrap();
    assert!(matches!(msg, RequestMessage::SubscribeOk(_)));

    // Read PUBLISH_DONE (forwarded by relay)
    let done_msg = sub_reader.read_message().await.unwrap();
    match done_msg {
        RequestMessage::PublishDone(publish_done) => {
            assert_eq!(publish_done.status_code, 0x2); // TRACK_ENDED
            assert_eq!(publish_done.stream_count, 1);
        }
        other => panic!("expected PublishDone, got {:?}", msg_name(&other)),
    }
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
        let (send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = RequestStreamReader::new(recv);
        let msg = reader.read_message().await.unwrap();
        assert!(matches!(msg, RequestMessage::Subscribe(_)));

        let mut writer = RequestStreamWriter::new(send);
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        writer.write_subscribe_ok(&ok).await.unwrap();

        for group_id in 0u64..3 {
            let mut uni = pub_conn2.open_uni().await.unwrap();
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
            status_code: 0x2,
            stream_count: 3,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        writer.write_publish_done(&done).await.unwrap();
    });

    // Subscriber
    let sub_conn = connect_client(addr, cert_der).await;
    let (sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let mut sub_writer = RequestStreamWriter::new(sub_send);
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    sub_writer.write_subscribe(&subscribe).await.unwrap();

    let mut sub_reader = RequestStreamReader::new(sub_recv);
    let msg = sub_reader.read_message().await.unwrap();
    assert!(matches!(msg, RequestMessage::SubscribeOk(_)));

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
    let done_msg = sub_reader.read_message().await.unwrap();
    match done_msg {
        RequestMessage::PublishDone(publish_done) => {
            assert_eq!(publish_done.stream_count, 3);
        }
        other => panic!("expected PublishDone, got {:?}", msg_name(&other)),
    }
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
        let (send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = RequestStreamReader::new(recv);
        let msg = reader.read_message().await.unwrap();
        assert!(matches!(msg, RequestMessage::Subscribe(_)));

        let mut writer = RequestStreamWriter::new(send);
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        writer.write_subscribe_ok(&ok).await.unwrap();

        // Send 2 groups after subscriber joins
        for group_id in 0u64..2 {
            let mut uni = pub_conn2.open_uni().await.unwrap();
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
            status_code: 0x2,
            stream_count: 2,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        writer.write_publish_done(&done).await.unwrap();
    });

    // Subscriber connects AFTER publisher is ready (simulating late join)
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let sub_conn = connect_client(addr, cert_der).await;
    let (sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let mut sub_writer = RequestStreamWriter::new(sub_send);
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
    sub_writer.write_subscribe(&subscribe).await.unwrap();

    let mut sub_reader = RequestStreamReader::new(sub_recv);
    let msg = sub_reader.read_message().await.unwrap();
    assert!(matches!(msg, RequestMessage::SubscribeOk(_)));

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
    let sub_conn = connect_client(addr, cert_der).await;
    let (sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
    let mut sub_writer = RequestStreamWriter::new(sub_send);
    let subscribe = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"nonexistent".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    sub_writer.write_subscribe(&subscribe).await.unwrap();

    // Should receive REQUEST_ERROR
    let mut reader = RequestStreamReader::new(sub_recv);
    let msg = reader.read_message().await.unwrap();
    match msg {
        RequestMessage::RequestError(err) => {
            assert_eq!(err.error_code, 0x10); // DOES_NOT_EXIST
        }
        other => panic!("expected RequestError, got {:?}", msg_name(&other)),
    }
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
        let (send1, recv1) = pub_conn2.accept_bi().await.unwrap();
        let mut reader1 = RequestStreamReader::new(recv1);
        let msg1 = reader1.read_message().await.unwrap();
        assert!(matches!(msg1, RequestMessage::Subscribe(_)));
        let mut writer1 = RequestStreamWriter::new(send1);
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        writer1.write_subscribe_ok(&ok).await.unwrap();

        // Accept second SUBSCRIBE
        let (send2, recv2) = pub_conn2.accept_bi().await.unwrap();
        let mut reader2 = RequestStreamReader::new(recv2);
        let msg2 = reader2.read_message().await.unwrap();
        assert!(matches!(msg2, RequestMessage::Subscribe(_)));
        let mut writer2 = RequestStreamWriter::new(send2);
        writer2.write_subscribe_ok(&ok).await.unwrap();

        // Small delay so both subscriptions are registered before sending
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Send 1 group with 1 object
        let mut uni = pub_conn2.open_uni().await.unwrap();
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
            status_code: 0x2,
            stream_count: 1,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        writer1.write_publish_done(&done).await.unwrap();
        writer2.write_publish_done(&done).await.unwrap();
    });

    // Helper to subscribe and receive objects
    async fn subscribe_and_receive(
        addr: SocketAddr,
        cert_der: rustls_pki_types::CertificateDer<'static>,
    ) -> Vec<u8> {
        let conn = connect_client(addr, cert_der).await;
        let (sub_send, sub_recv) = conn.open_bi().await.unwrap();
        let mut sub_writer = RequestStreamWriter::new(sub_send);
        let subscribe = SubscribeMessage {
            request_id: 0,
            required_request_id_delta: 0,
            track_namespace: TrackNamespace {
                fields: vec![b"example".to_vec()],
            },
            track_name: b"video".to_vec(),
            parameters: vec![],
        };
        sub_writer.write_subscribe(&subscribe).await.unwrap();

        let mut reader = RequestStreamReader::new(sub_recv);
        let _msg = reader.read_message().await.unwrap();

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
        let (send_v, recv_v) = pub_conn2.accept_bi().await.unwrap();
        let mut reader_v = RequestStreamReader::new(recv_v);
        let _ = reader_v.read_message().await.unwrap();
        let mut writer_v = RequestStreamWriter::new(send_v);
        let ok_v = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        writer_v.write_subscribe_ok(&ok_v).await.unwrap();

        // Accept SUBSCRIBE for audio (alias=2)
        let (send_a, recv_a) = pub_conn2.accept_bi().await.unwrap();
        let mut reader_a = RequestStreamReader::new(recv_a);
        let _ = reader_a.read_message().await.unwrap();
        let mut writer_a = RequestStreamWriter::new(send_a);
        let ok_a = SubscribeOkMessage {
            track_alias: 2,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        writer_a.write_subscribe_ok(&ok_a).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send video object
        let mut uni_v = pub_conn2.open_uni().await.unwrap();
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
        let mut uni_a = pub_conn2.open_uni().await.unwrap();
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
            status_code: 0x2,
            stream_count: 1,
            reason_phrase: moqt_core::primitives::reason_phrase::ReasonPhrase { value: vec![] },
        };
        writer_v.write_publish_done(&done).await.unwrap();
        writer_a.write_publish_done(&done).await.unwrap();
    });

    // Subscriber: subscribe to both tracks
    let sub_conn = connect_client(addr, cert_der).await;

    // Subscribe to video
    let (sub_send_v, sub_recv_v) = sub_conn.open_bi().await.unwrap();
    let mut sub_writer_v = RequestStreamWriter::new(sub_send_v);
    let sub_v = SubscribeMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"video".to_vec(),
        parameters: vec![],
    };
    sub_writer_v.write_subscribe(&sub_v).await.unwrap();
    let mut reader_v = RequestStreamReader::new(sub_recv_v);
    let _ = reader_v.read_message().await.unwrap(); // SUBSCRIBE_OK

    // Subscribe to audio
    let (sub_send_a, sub_recv_a) = sub_conn.open_bi().await.unwrap();
    let mut sub_writer_a = RequestStreamWriter::new(sub_send_a);
    let sub_a = SubscribeMessage {
        request_id: 2,
        required_request_id_delta: 0,
        track_namespace: TrackNamespace {
            fields: vec![b"example".to_vec()],
        },
        track_name: b"audio".to_vec(),
        parameters: vec![],
    };
    sub_writer_a.write_subscribe(&sub_a).await.unwrap();
    let mut reader_a = RequestStreamReader::new(sub_recv_a);
    let _ = reader_a.read_message().await.unwrap(); // SUBSCRIBE_OK

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

/// 6.3: Subscriber disconnect -- relay cleans up, publisher continues
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
        let (send, recv) = pub_conn2.accept_bi().await.unwrap();
        let mut reader = RequestStreamReader::new(recv);
        let _ = reader.read_message().await.unwrap();
        let mut writer = RequestStreamWriter::new(send);
        let ok = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        writer.write_subscribe_ok(&ok).await.unwrap();

        // Send a few groups
        for group_id in 0u64..3 {
            let mut uni = pub_conn2.open_uni().await.unwrap();
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
            pub_conn2.close_reason().is_none(),
            "publisher should still be connected"
        );
    });

    // Subscriber connects, subscribes, receives 1 group, then disconnects
    {
        let sub_conn = connect_client(addr, cert_der).await;
        let (sub_send, sub_recv) = sub_conn.open_bi().await.unwrap();
        let mut sub_writer = RequestStreamWriter::new(sub_send);
        let subscribe = SubscribeMessage {
            request_id: 0,
            required_request_id_delta: 0,
            track_namespace: TrackNamespace {
                fields: vec![b"example".to_vec()],
            },
            track_name: b"video".to_vec(),
            parameters: vec![],
        };
        sub_writer.write_subscribe(&subscribe).await.unwrap();

        let mut reader = RequestStreamReader::new(sub_recv);
        let _ = reader.read_message().await.unwrap(); // SUBSCRIBE_OK

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

/// Helper to get a human-readable name for a RequestMessage variant (for panic messages).
fn msg_name(msg: &RequestMessage) -> &'static str {
    match msg {
        RequestMessage::PublishNamespace(_) => "PublishNamespace",
        RequestMessage::Subscribe(_) => "Subscribe",
        RequestMessage::SubscribeOk(_) => "SubscribeOk",
        RequestMessage::RequestOk(_) => "RequestOk",
        RequestMessage::RequestError(_) => "RequestError",
        RequestMessage::PublishDone(_) => "PublishDone",
    }
}

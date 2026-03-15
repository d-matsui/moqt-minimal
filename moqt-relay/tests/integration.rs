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
use moqt_core::message::publish_namespace::PublishNamespaceMessage;
use moqt_core::message::request_ok::RequestOkMessage;
use moqt_core::message::setup::{SetupMessage, SetupOption};
use moqt_core::message::subscribe::SubscribeMessage;
use moqt_core::message::subscribe_ok::SubscribeOkMessage;
use moqt_core::session::control_stream::ControlStreamReader;
use moqt_core::session::quic_config;
use moqt_core::wire::track_namespace::TrackNamespace;

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
async fn start_relay() -> (quinn::Endpoint, SocketAddr, rustls_pki_types::CertificateDer<'static>) {
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
    let mut client_endpoint =
        quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
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
    setup.encode(&mut buf);
    ctrl_send.write_all(&buf).await.unwrap();

    // Accept relay's control stream and read SETUP
    let recv = connection.accept_uni().await.unwrap();
    let mut reader = ControlStreamReader::new(recv);
    let _relay_setup = reader.read_setup().await.unwrap();

    connection
}

/// Helper: send PUBLISH_NAMESPACE and receive REQUEST_OK
async fn publish_namespace(
    conn: &quinn::Connection,
    namespace: TrackNamespace,
) {
    let (mut send, recv) = conn.open_bi().await.unwrap();
    let msg = PublishNamespaceMessage {
        request_id: 0,
        required_request_id_delta: 0,
        track_namespace: namespace,
    };
    let mut buf = Vec::new();
    msg.encode(&mut buf);
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
            let setup = SetupMessage { setup_options: vec![] };
            let mut buf = Vec::new();
            setup.encode(&mut buf);
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
        };
        let mut buf = Vec::new();
        ok.encode(&mut buf);
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
    subscribe.encode(&mut buf);
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
        };
        let mut buf = Vec::new();
        ok.encode(&mut buf);
        send.write_all(&buf).await.unwrap();

        // Send a subgroup with 2 objects
        let mut uni = pub_conn2.open_uni().await.unwrap();
        let header = SubgroupHeader {
            track_alias: 1,
            group_id: 0,
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
    subscribe.encode(&mut buf);
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

    let obj0 = ObjectHeader::decode(&mut data_slice).unwrap();
    assert_eq!(obj0.object_id_delta, 0);
    assert_eq!(obj0.payload_length, 5);
    let payload0 = &data_slice[..5];
    data_slice = &data_slice[5..];
    assert_eq!(payload0, b"hello");

    let obj1 = ObjectHeader::decode(&mut data_slice).unwrap();
    assert_eq!(obj1.object_id_delta, 0);
    assert_eq!(obj1.payload_length, 5);
    let payload1 = &data_slice[..5];
    assert_eq!(payload1, b"world");
}

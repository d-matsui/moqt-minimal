use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use quinn::{Connection, Endpoint};
use tokio::sync::Mutex;

use moqt_core::message::publish_done::PublishDoneMessage;
use moqt_core::message::publish_namespace::PublishNamespaceMessage;
use moqt_core::message::request_error::RequestErrorMessage;
use moqt_core::message::request_ok::RequestOkMessage;
use moqt_core::message::setup::SetupMessage;
use moqt_core::message::subscribe::SubscribeMessage;
use moqt_core::message::subscribe_ok::SubscribeOkMessage;
use moqt_core::message::{MSG_PUBLISH_NAMESPACE, MSG_SUBSCRIBE};
use moqt_core::session::control_stream::ControlStreamReader;
use moqt_core::session::request_id::RequestIdAllocator;
use moqt_core::wire::reason_phrase::ReasonPhrase;
use moqt_core::wire::track_namespace::TrackNamespace;
use moqt_core::wire::varint::decode_varint;

type SessionId = u64;

pub struct Relay {
    endpoint: Endpoint,
    state: Arc<Mutex<RelayState>>,
}

struct RelayState {
    next_session_id: u64,
    sessions: HashMap<SessionId, SessionState>,
    namespace_publishers: HashMap<TrackNamespace, SessionId>,
    subscriptions: Vec<SubscriptionEntry>,
    request_id_alloc: RequestIdAllocator,
}

struct SessionState {
    connection: Connection,
}

struct SubscriptionEntry {
    subscriber_session: SessionId,
    publisher_session: SessionId,
    track_namespace: TrackNamespace,
    track_name: Vec<u8>,
    publisher_track_alias: u64,
    #[allow(dead_code)]
    subscriber_track_alias: u64,
    subscriber_bidi_send: Arc<Mutex<quinn::SendStream>>,
}

impl Relay {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            state: Arc::new(Mutex::new(RelayState {
                next_session_id: 0,
                sessions: HashMap::new(),
                namespace_publishers: HashMap::new(),
                subscriptions: Vec::new(),
                request_id_alloc: RequestIdAllocator::server(),
            })),
        }
    }

    pub async fn run(&self) -> io::Result<()> {
        while let Some(incoming) = self.endpoint.accept().await {
            let state = self.state.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(incoming, state).await {
                    eprintln!("connection error: {e}");
                }
            });
        }
        Ok(())
    }
}

async fn handle_connection(
    incoming: quinn::Incoming,
    state: Arc<Mutex<RelayState>>,
) -> io::Result<()> {
    let connection = incoming.await.map_err(io::Error::other)?;

    let session_id = {
        let mut s = state.lock().await;
        let id = s.next_session_id;
        s.next_session_id += 1;
        s.sessions.insert(
            id,
            SessionState {
                connection: connection.clone(),
            },
        );
        id
    };

    // SETUP exchange
    let mut our_ctrl_send = connection.open_uni().await.map_err(io::Error::other)?;
    let server_setup = SetupMessage {
        setup_options: vec![],
    };
    let mut setup_buf = Vec::new();
    server_setup.encode(&mut setup_buf);
    our_ctrl_send
        .write_all(&setup_buf)
        .await
        .map_err(io::Error::other)?;

    let peer_ctrl_recv = connection.accept_uni().await.map_err(io::Error::other)?;
    let mut ctrl_reader = ControlStreamReader::new(peer_ctrl_recv);
    let _peer_setup = ctrl_reader.read_setup().await?;

    // Handle both bidi streams and uni streams concurrently
    loop {
        tokio::select! {
            bidi = connection.accept_bi() => {
                match bidi {
                    Ok((send, recv)) => {
                        let state = state.clone();
                        let conn = connection.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_bidi_stream(session_id, send, recv, state, conn).await {
                                eprintln!("bidi stream error: {e}");
                            }
                        });
                    }
                    Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                    Err(quinn::ConnectionError::LocallyClosed) => break,
                    Err(e) => return Err(io::Error::other(e)),
                }
            }
            uni = connection.accept_uni() => {
                match uni {
                    Ok(recv) => {
                        let state = state.clone();
                        let sid = session_id;
                        tokio::spawn(async move {
                            if let Err(e) = handle_incoming_uni(sid, recv, state).await {
                                eprintln!("uni stream error: {e}");
                            }
                        });
                    }
                    Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                    Err(quinn::ConnectionError::LocallyClosed) => break,
                    Err(e) => return Err(io::Error::other(e)),
                }
            }
        }
    }

    // Clean up
    {
        let mut s = state.lock().await;
        s.sessions.remove(&session_id);
        s.namespace_publishers.retain(|_, v| *v != session_id);
        s.subscriptions.retain(|sub| {
            sub.subscriber_session != session_id && sub.publisher_session != session_id
        });
    }

    Ok(())
}

/// Read exactly `n` bytes from a RecvStream.
async fn read_exact(recv: &mut quinn::RecvStream, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    let mut filled = 0;
    while filled < n {
        match recv.read(&mut buf[filled..]).await {
            Ok(Some(read)) => filled += read,
            Ok(None) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("stream ended after {filled}/{n} bytes"),
                ))
            }
            Err(e) => return Err(io::Error::other(e)),
        }
    }
    Ok(buf)
}

/// Read a varint from a RecvStream. Returns (value, raw_bytes).
async fn read_varint_from_stream(recv: &mut quinn::RecvStream) -> io::Result<(u64, Vec<u8>)> {
    let first = read_exact(recv, 1).await?;
    let byte = first[0];
    let total_len = if byte & 0x80 == 0 {
        1
    } else if byte & 0xc0 == 0x80 {
        2
    } else if byte & 0xe0 == 0xc0 {
        3
    } else if byte & 0xf0 == 0xe0 {
        4
    } else if byte & 0xf8 == 0xf0 {
        5
    } else if byte & 0xfc == 0xf8 {
        6
    } else if byte == 0xfc {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid varint code point 0xFC",
        ));
    } else if byte == 0xfe {
        8
    } else {
        9
    };

    let mut raw = vec![0u8; total_len];
    raw[0] = byte;
    if total_len > 1 {
        let rest = read_exact(recv, total_len - 1).await?;
        raw[1..].copy_from_slice(&rest);
    }

    let mut slice = raw.as_slice();
    let value = decode_varint(&mut slice)?;
    Ok((value, raw))
}

/// Handle an incoming unidirectional stream (Object data from a publisher).
/// Streams Objects to subscribers one at a time without buffering the entire stream.
async fn handle_incoming_uni(
    sender_session: SessionId,
    mut recv: quinn::RecvStream,
    state: Arc<Mutex<RelayState>>,
) -> io::Result<()> {
    // Read SUBGROUP_HEADER: Type (varint) + Track Alias (varint) + Group ID (varint)
    let (stream_type, type_bytes) = read_varint_from_stream(&mut recv).await?;
    let (track_alias, alias_bytes) = read_varint_from_stream(&mut recv).await?;
    let (_group_id, group_bytes) = read_varint_from_stream(&mut recv).await?;

    // Verify it's a valid subgroup header type (bit 4 set)
    if stream_type & 0x10 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected SUBGROUP_HEADER, got type 0x{stream_type:X}"),
        ));
    }

    // Find all subscribers and open downstream streams
    let subscriber_streams: Vec<quinn::SendStream> = {
        let s = state.lock().await;
        let sub_conns: Vec<Connection> = s
            .subscriptions
            .iter()
            .filter(|sub| {
                sub.publisher_session == sender_session && sub.publisher_track_alias == track_alias
            })
            .filter_map(|sub| {
                s.sessions
                    .get(&sub.subscriber_session)
                    .map(|sess| sess.connection.clone())
            })
            .collect();
        drop(s);

        let mut streams = Vec::new();
        for conn in sub_conns {
            match conn.open_uni().await {
                Ok(stream) => streams.push(stream),
                Err(e) => eprintln!("failed to open uni to subscriber: {e}"),
            }
        }
        streams
    };

    if subscriber_streams.is_empty() {
        // No subscribers, drain the stream
        let mut tmp = vec![0u8; 4096];
        loop {
            match recv.read(&mut tmp).await {
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(_) => break,
            }
        }
        return Ok(());
    }

    // Write SUBGROUP_HEADER to all subscriber streams
    let mut header_bytes = Vec::new();
    header_bytes.extend_from_slice(&type_bytes);
    header_bytes.extend_from_slice(&alias_bytes);
    header_bytes.extend_from_slice(&group_bytes);

    let subscriber_streams: Vec<Arc<Mutex<quinn::SendStream>>> = subscriber_streams
        .into_iter()
        .map(|s| Arc::new(Mutex::new(s)))
        .collect();

    for stream in &subscriber_streams {
        stream
            .lock()
            .await
            .write_all(&header_bytes)
            .await
            .map_err(io::Error::other)?;
    }

    // Stream objects one by one
    loop {
        // Read Object ID Delta (varint)
        let delta_result = read_varint_from_stream(&mut recv).await;
        let (_delta, delta_bytes) = match delta_result {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break, // stream FIN
            Err(e) => return Err(e),
        };

        // Read Payload Length (varint)
        let (payload_len, len_bytes) = read_varint_from_stream(&mut recv).await?;

        // Read Payload
        let payload = read_exact(&mut recv, payload_len as usize).await?;

        // Forward to all subscribers immediately
        for stream in &subscriber_streams {
            let mut s = stream.lock().await;
            let _ = s.write_all(&delta_bytes).await;
            let _ = s.write_all(&len_bytes).await;
            let _ = s.write_all(&payload).await;
        }
    }

    // Close all subscriber streams with FIN
    for stream in subscriber_streams {
        let mut s = stream.lock().await;
        let _ = s.finish();
    }

    Ok(())
}

async fn handle_bidi_stream(
    session_id: SessionId,
    mut send: quinn::SendStream,
    recv: quinn::RecvStream,
    state: Arc<Mutex<RelayState>>,
    connection: Connection,
) -> io::Result<()> {
    let mut reader = ControlStreamReader::new(recv);
    let msg_bytes = reader.read_message_bytes().await?;
    let mut slice = msg_bytes.as_slice();
    let msg_type = decode_varint(&mut slice)?;

    let mut slice = msg_bytes.as_slice();

    match msg_type {
        MSG_PUBLISH_NAMESPACE => {
            let msg = PublishNamespaceMessage::decode(&mut slice)?;
            {
                let mut s = state.lock().await;
                s.namespace_publishers
                    .insert(msg.track_namespace.clone(), session_id);
            }
            let ok = RequestOkMessage {};
            let mut buf = Vec::new();
            ok.encode(&mut buf);
            send.write_all(&buf).await.map_err(io::Error::other)?;
        }
        MSG_SUBSCRIBE => {
            let msg = SubscribeMessage::decode(&mut slice)?;
            handle_subscribe(session_id, msg, send, state, connection).await?;
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected message type on bidi stream: 0x{msg_type:X}"),
            ));
        }
    }

    Ok(())
}

async fn handle_subscribe(
    subscriber_session: SessionId,
    msg: SubscribeMessage,
    subscriber_send: quinn::SendStream,
    state: Arc<Mutex<RelayState>>,
    _subscriber_conn: Connection,
) -> io::Result<()> {
    let subscriber_send = Arc::new(Mutex::new(subscriber_send));

    // Find publisher for this namespace
    let (publisher_session_id, publisher_conn) = {
        let s = state.lock().await;
        let ns = &msg.track_namespace;
        let pub_id = s
            .namespace_publishers
            .iter()
            .find_map(|(registered_ns, sid)| {
                if registered_ns.fields.len() <= ns.fields.len()
                    && registered_ns
                        .fields
                        .iter()
                        .zip(ns.fields.iter())
                        .all(|(a, b)| a == b)
                {
                    Some(*sid)
                } else {
                    None
                }
            });
        match pub_id {
            Some(id) => {
                let conn = s
                    .sessions
                    .get(&id)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, "publisher session gone")
                    })?
                    .connection
                    .clone();
                (id, conn)
            }
            None => {
                let err = RequestErrorMessage {
                    error_code: 0x10,
                    retry_interval: 0,
                    reason_phrase: ReasonPhrase {
                        value: b"no publisher for namespace".to_vec(),
                    },
                };
                let mut buf = Vec::new();
                err.encode(&mut buf);
                subscriber_send
                    .lock()
                    .await
                    .write_all(&buf)
                    .await
                    .map_err(io::Error::other)?;
                return Ok(());
            }
        }
    };

    // Forward SUBSCRIBE to publisher
    let (mut pub_send, pub_recv) = publisher_conn.open_bi().await.map_err(io::Error::other)?;

    let relay_request_id = {
        let mut s = state.lock().await;
        s.request_id_alloc.allocate()
    };
    let upstream_subscribe = SubscribeMessage {
        request_id: relay_request_id,
        required_request_id_delta: 0,
        track_namespace: msg.track_namespace.clone(),
        track_name: msg.track_name.clone(),
        parameters: msg.parameters.clone(),
    };
    let mut buf = Vec::new();
    upstream_subscribe.encode(&mut buf);
    pub_send.write_all(&buf).await.map_err(io::Error::other)?;

    // Read SUBSCRIBE_OK from publisher
    let mut pub_reader = ControlStreamReader::new(pub_recv);
    let ok_bytes = pub_reader.read_message_bytes().await?;
    let mut ok_slice = ok_bytes.as_slice();
    let subscribe_ok = SubscribeOkMessage::decode(&mut ok_slice)?;

    let track_alias = subscribe_ok.track_alias;

    // Record subscription
    {
        let mut s = state.lock().await;
        s.subscriptions.push(SubscriptionEntry {
            subscriber_session,
            publisher_session: publisher_session_id,
            track_namespace: msg.track_namespace.clone(),
            track_name: msg.track_name.clone(),
            publisher_track_alias: track_alias,
            subscriber_track_alias: track_alias,
            subscriber_bidi_send: subscriber_send.clone(),
        });
    }

    // Forward SUBSCRIBE_OK to subscriber
    let mut ok_buf = Vec::new();
    subscribe_ok.encode(&mut ok_buf);
    subscriber_send
        .lock()
        .await
        .write_all(&ok_buf)
        .await
        .map_err(io::Error::other)?;

    // Wait for PUBLISH_DONE from publisher, forward to all subscribers
    let done_bytes = pub_reader.read_message_bytes().await?;
    let mut done_slice = done_bytes.as_slice();
    let _publish_done = PublishDoneMessage::decode(&mut done_slice)?;

    let subs_to_notify: Vec<Arc<Mutex<quinn::SendStream>>> = {
        let s = state.lock().await;
        s.subscriptions
            .iter()
            .filter(|sub| {
                sub.publisher_session == publisher_session_id
                    && sub.track_namespace == msg.track_namespace
                    && sub.track_name == msg.track_name
            })
            .map(|sub| sub.subscriber_bidi_send.clone())
            .collect()
    };

    for send in subs_to_notify {
        let _ = send.lock().await.write_all(&done_bytes).await;
    }

    Ok(())
}

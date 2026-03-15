use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use quinn::{Connection, Endpoint};
use tokio::sync::Mutex;

use moqt_core::data::subgroup_header::SubgroupHeader;
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
    /// Track which publishers already have a forwarding task running.
    /// Key: publisher session ID. Prevents spawning duplicate forward_objects tasks.
    forwarding_publishers: std::collections::HashSet<SessionId>,
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
    subscriber_track_alias: u64,
    /// Send half of the subscriber's bidi stream, for sending PUBLISH_DONE.
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
                forwarding_publishers: std::collections::HashSet::new(),
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
    let connection = incoming
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

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
    let mut our_ctrl_send = connection
        .open_uni()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let server_setup = SetupMessage {
        setup_options: vec![],
    };
    let mut setup_buf = Vec::new();
    server_setup.encode(&mut setup_buf);
    our_ctrl_send
        .write_all(&setup_buf)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let peer_ctrl_recv = connection
        .accept_uni()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let mut ctrl_reader = ControlStreamReader::new(peer_ctrl_recv);
    let _peer_setup = ctrl_reader.read_setup().await?;

    // Handle bidi streams
    loop {
        let bidi = connection.accept_bi().await;
        match bidi {
            Ok((send, recv)) => {
                let state = state.clone();
                let conn = connection.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        handle_bidi_stream(session_id, send, recv, state, conn).await
                    {
                        eprintln!("bidi stream error: {e}");
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
            Err(quinn::ConnectionError::LocallyClosed) => break,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    // Clean up
    {
        let mut s = state.lock().await;
        s.sessions.remove(&session_id);
        s.namespace_publishers.retain(|_, v| *v != session_id);
        s.forwarding_publishers.remove(&session_id);
        s.subscriptions.retain(|sub| {
            sub.subscriber_session != session_id && sub.publisher_session != session_id
        });
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
            send.write_all(&buf)
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
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
                    error_code: 0x10, // DOES_NOT_EXIST
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
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                return Ok(());
            }
        }
    };

    // Forward SUBSCRIBE to publisher
    let (mut pub_send, pub_recv) = publisher_conn
        .open_bi()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let relay_request_id = {
        let mut s = state.lock().await;
        s.request_id_alloc.next()
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
    pub_send
        .write_all(&buf)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

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

        // Start forwarding task for this publisher if not already running
        if !s.forwarding_publishers.contains(&publisher_session_id) {
            s.forwarding_publishers.insert(publisher_session_id);
            let state_clone = state.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    forward_objects(publisher_session_id, state_clone).await
                {
                    eprintln!("object forwarding error: {e}");
                }
            });
        }
    }

    // Forward SUBSCRIBE_OK to subscriber
    let mut ok_buf = Vec::new();
    subscribe_ok.encode(&mut ok_buf);
    subscriber_send
        .lock()
        .await
        .write_all(&ok_buf)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    // Wait for PUBLISH_DONE from publisher, forward to all subscribers of this track
    let done_bytes = pub_reader.read_message_bytes().await?;
    let mut done_slice = done_bytes.as_slice();
    let _publish_done = PublishDoneMessage::decode(&mut done_slice)?;

    // Forward to all subscribers for this publisher + track
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
        let _ = send
            .lock()
            .await
            .write_all(&done_bytes)
            .await;
    }

    Ok(())
}

/// Forward Object streams from a publisher to all subscribers.
/// One task per publisher — reads uni streams and fans out to all matching subscribers.
async fn forward_objects(
    publisher_session: SessionId,
    state: Arc<Mutex<RelayState>>,
) -> io::Result<()> {
    let publisher_conn = {
        let s = state.lock().await;
        s.sessions
            .get(&publisher_session)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "publisher gone"))?
            .connection
            .clone()
    };

    loop {
        let uni = publisher_conn.accept_uni().await;
        match uni {
            Ok(mut recv) => {
                let state = state.clone();
                let pub_session = publisher_session;
                tokio::spawn(async move {
                    if let Err(e) =
                        forward_one_subgroup(&mut recv, pub_session, state).await
                    {
                        eprintln!("subgroup forward error: {e}");
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
            Err(quinn::ConnectionError::LocallyClosed) => break,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    Ok(())
}

async fn forward_one_subgroup(
    recv: &mut quinn::RecvStream,
    publisher_session: SessionId,
    state: Arc<Mutex<RelayState>>,
) -> io::Result<()> {
    let mut all_data = Vec::new();
    let mut tmp = vec![0u8; 4096];
    loop {
        match recv.read(&mut tmp).await {
            Ok(Some(n)) => all_data.extend_from_slice(&tmp[..n]),
            Ok(None) => break,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    if all_data.is_empty() {
        return Ok(());
    }

    // Decode header to get track alias
    let mut check = all_data.as_slice();
    let header = SubgroupHeader::decode(&mut check)?;

    // Find all subscribers for this publisher + track alias
    let subscriber_conns: Vec<(SessionId, Connection)> = {
        let s = state.lock().await;
        s.subscriptions
            .iter()
            .filter(|sub| {
                sub.publisher_session == publisher_session
                    && sub.publisher_track_alias == header.track_alias
            })
            .filter_map(|sub| {
                s.sessions
                    .get(&sub.subscriber_session)
                    .map(|sess| (sub.subscriber_session, sess.connection.clone()))
            })
            .collect()
    };

    // Forward to each subscriber
    for (_sub_id, sub_conn) in subscriber_conns {
        let data = all_data.clone();
        tokio::spawn(async move {
            let result = async {
                let mut sub_send = sub_conn
                    .open_uni()
                    .await
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                sub_send
                    .write_all(&data)
                    .await
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                sub_send
                    .finish()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                Ok::<(), io::Error>(())
            }
            .await;
            if let Err(e) = result {
                eprintln!("forward to subscriber error: {e}");
            }
        });
    }

    Ok(())
}

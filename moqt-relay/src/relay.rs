//! # relay: MOQT relay server implementation
//!
//! This module implements the core logic of the MOQT relay.
//!
//! ## Architecture overview
//!
//! ```text
//! Publisher ──QUIC conn──→ [Relay Server] ←──QUIC conn── Subscriber
//!   │                              │                              │
//!   ├─ SETUP exchange              │                SETUP exchange─┤
//!   ├─ PUBLISH_NAMESPACE register  │                              │
//!   │                              ├─ SUBSCRIBE forward ─────────→│
//!   │                 SUBSCRIBE_OK ←┤                              │
//!   ├─ Data stream (uni) ────────→├─ Data stream relay ──────────→│
//!   └─ PUBLISH_DONE ─────────────→├─ PUBLISH_DONE forward ──────→│
//! ```
//!
//! ## Per-connection processing flow
//! 1. Accept a new QUIC connection and assign a session ID
//! 2. Exchange SETUP messages
//! 3. Process control messages on bidi streams:
//!    - PUBLISH_NAMESPACE: register namespace and respond with REQUEST_OK
//!    - SUBSCRIBE: forward to publisher and relay response back to subscriber
//! 4. Relay data on uni streams:
//!    - Identify the subscription from the Track Alias in SubgroupHeader
//!    - Read objects one by one and forward them to subscribers
//!
//! ## Shared state (RelayState)
//! State shared across all sessions is managed via `Arc<Mutex<RelayState>>`.
//! - `sessions`: list of active sessions
//! - `namespace_publishers`: namespace-to-publisher mapping
//! - `subscriptions`: list of active subscriptions

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};

use quinn::{Connection, Endpoint};
use tokio::sync::Mutex;

use moqt_core::message::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::message::request_error::RequestErrorMessage;
use moqt_core::message::request_ok::RequestOkMessage;
use moqt_core::message::subscribe::SubscribeMessage;
use moqt_core::primitives::reason_phrase::ReasonPhrase;
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::data_stream::{DataStreamReader, DataStreamWriter};
use moqt_core::session::moqt_session::MoqtSession;
use moqt_core::session::request_id::RequestIdAllocator;
use moqt_core::session::request_stream::{
    RequestMessage, RequestStreamReader, RequestStreamWriter,
};

/// Unique identifier for a session. Assigned sequentially per connection.
type SessionId = u64;

/// MOQT relay server. Holds a QUIC endpoint and accepts connections.
pub struct Relay {
    endpoint: Endpoint,
    /// State shared across all sessions. Protected by a Mutex.
    state: Arc<Mutex<RelayState>>,
}

/// Shared relay state. Manages sessions, namespace registrations, and subscriptions.
struct RelayState {
    /// Next session ID to assign
    next_session_id: u64,
    /// List of active sessions
    sessions: HashMap<SessionId, SessionState>,
    /// Namespace-to-publisher session ID mapping.
    /// Used to forward subscriber SUBSCRIBE messages to the appropriate publisher.
    namespace_publishers: HashMap<TrackNamespace, SessionId>,
    /// List of active subscriptions. Used to identify relay destinations for data streams.
    subscriptions: Vec<SubscriptionEntry>,
    /// Server-side request ID allocator (generates odd IDs).
    /// Assigns new IDs when forwarding SUBSCRIBE to publishers.
    request_id_alloc: RequestIdAllocator,
}

/// Per-session state. Holds a reference to the QUIC connection.
struct SessionState {
    connection: Connection,
}

/// Subscription entry. Records the mapping between a subscriber and a publisher.
struct SubscriptionEntry {
    /// Session ID of the subscriber that requested the subscription
    subscriber_session: SessionId,
    /// Session ID of the publisher delivering data
    publisher_session: SessionId,
    /// Track namespace of the subscription
    track_namespace: TrackNamespace,
    /// Track name of the subscription
    track_name: Vec<u8>,
    /// Track alias assigned by the publisher.
    /// Included in the SubgroupHeader of data streams,
    /// so this value identifies which subscription the data belongs to.
    publisher_track_alias: u64,
    #[allow(dead_code)]
    subscriber_track_alias: u64,
    /// Write side of the subscriber's bidi stream.
    /// Used to forward PUBLISH_DONE.
    subscriber_bidi_send: Arc<Mutex<RequestStreamWriter>>,
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

    /// Main loop of the relay server.
    /// Accepts new QUIC connections and processes each in an async task.
    pub async fn run(&self) -> Result<()> {
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

/// Process a single QUIC connection (session).
/// After the SETUP exchange, process bidi and uni streams concurrently.
async fn handle_connection(incoming: quinn::Incoming, state: Arc<Mutex<RelayState>>) -> Result<()> {
    let connection = incoming.await?;

    // Assign a session ID and register in the session list
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

    // === SETUP exchange ===
    let session = MoqtSession::accept(connection.clone()).await?;
    let connection = session.connection().clone();

    // === Main loop: process bidi and uni streams concurrently ===
    // Use tokio::select! to await both simultaneously and process whichever arrives first.
    // bidi: control messages (PUBLISH_NAMESPACE, SUBSCRIBE)
    // uni: data streams (SubgroupHeader + Objects)
    loop {
        tokio::select! {
            bidi = connection.accept_bi() => {
                match bidi {
                    Ok((send, recv)) => {
                        let state = state.clone();
                        let conn = connection.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_request_stream(session_id, send, recv, state, conn).await {
                                eprintln!("bidi stream error: {e}");
                            }
                        });
                    }
                    Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                    Err(quinn::ConnectionError::LocallyClosed) => break,
                    Err(e) => return Err(e.into()),
                }
            }
            uni = connection.accept_uni() => {
                match uni {
                    Ok(recv) => {
                        let state = state.clone();
                        let sid = session_id;
                        tokio::spawn(async move {
                            if let Err(e) = handle_data_stream(sid, recv, state).await {
                                eprintln!("uni stream error: {e}");
                            }
                        });
                    }
                    Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                    Err(quinn::ConnectionError::LocallyClosed) => break,
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }

    // === Cleanup on disconnect ===
    // Remove all state associated with this session.
    // Without removing namespace registrations and subscription entries,
    // the relay would attempt to forward to disconnected sessions.
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

/// Process a unidirectional data stream from a publisher and relay it to subscribers.
///
/// ## Relay flow
/// 1. Read the SubgroupHeader and identify the target subscription from the Track Alias
/// 2. Open new uni streams to all subscribers
/// 3. Forward the SubgroupHeader to subscribers
/// 4. Read objects one by one and immediately forward them to subscribers
///    (relay with low latency without buffering the entire stream)
/// 5. Propagate stream termination (FIN) to subscribers
async fn handle_data_stream(
    sender_session: SessionId,
    recv: quinn::RecvStream,
    state: Arc<Mutex<RelayState>>,
) -> Result<()> {
    let mut data_reader = DataStreamReader::new(recv);

    // === Read and validate SubgroupHeader ===
    let (header, header_bytes) = data_reader.read_subgroup_header().await?;
    let track_alias = header.track_alias;

    // === Identify subscribers and open downstream streams ===
    // Find matching subscriptions by Track Alias and sender session,
    // then open uni streams to each subscriber
    let subscriber_writers: Vec<DataStreamWriter> = {
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
        // Release the lock before opening streams (since it involves await)
        drop(s);

        let mut writers = Vec::new();
        for conn in sub_conns {
            match conn.open_uni().await {
                Ok(stream) => writers.push(DataStreamWriter::new(stream)),
                Err(e) => eprintln!("failed to open uni to subscriber: {e}"),
            }
        }
        writers
    };

    // If there are no subscribers, drain the stream
    if subscriber_writers.is_empty() {
        while let Ok(Some(_)) = data_reader.read_object().await {}
        return Ok(());
    }

    // === Forward SubgroupHeader ===
    // Write the raw bytes as-is to subscribers
    let subscriber_writers: Vec<Arc<Mutex<DataStreamWriter>>> = subscriber_writers
        .into_iter()
        .map(|w| Arc::new(Mutex::new(w)))
        .collect();

    for writer in &subscriber_writers {
        writer.lock().await.write_raw(&header_bytes).await?;
    }

    // === Relay objects incrementally ===
    // Read objects one by one and immediately forward to all subscribers.
    // No buffering, so memory usage stays low even for large streams.
    while let Some((_obj, payload, obj_header_bytes)) = data_reader.read_object().await? {
        // Forward immediately to all subscribers
        // Continue forwarding to other subscribers even if an error occurs
        for writer in &subscriber_writers {
            let mut w = writer.lock().await;
            let _ = w.write_raw(&obj_header_bytes).await;
            let _ = w.write_raw(&payload).await;
        }
    }

    // === Propagate stream termination ===
    // When the publisher's stream ends,
    // send FIN to subscriber streams via finish()
    for writer in subscriber_writers {
        let mut w = writer.lock().await;
        let _ = w.finish();
    }

    Ok(())
}

/// Handle a request (bidi) stream.
/// Read the first message and dispatch to the appropriate handler.
async fn handle_request_stream(
    session_id: SessionId,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    state: Arc<Mutex<RelayState>>,
    connection: Connection,
) -> Result<()> {
    let mut reader = RequestStreamReader::new(recv);
    let mut writer = RequestStreamWriter::new(send);
    let msg = reader.read_message().await?;

    match msg {
        RequestMessage::PublishNamespace(pub_ns) => {
            {
                let mut s = state.lock().await;
                s.namespace_publishers
                    .insert(pub_ns.track_namespace.clone(), session_id);
            }
            let ok = RequestOkMessage {};
            writer.write_request_ok(&ok).await?;
        }
        RequestMessage::Subscribe(subscribe) => {
            handle_subscribe(session_id, subscribe, writer, state, connection).await?;
        }
        _ => {
            bail!("unexpected message on request stream");
        }
    }

    Ok(())
}

/// Handle a SUBSCRIBE message.
///
/// 1. Check subscription filter (only NextGroupStart supported)
/// 2. Find publisher by namespace (prefix match)
/// 3. Forward SUBSCRIBE to publisher (with relay-assigned request ID)
/// 4. Receive SUBSCRIBE_OK from publisher
/// 5. Record subscription entry (used for data stream relay)
/// 6. Forward SUBSCRIBE_OK to subscriber
/// 7. Wait for PUBLISH_DONE from publisher and forward to subscriber
async fn handle_subscribe(
    subscriber_session: SessionId,
    msg: SubscribeMessage,
    subscriber_writer: RequestStreamWriter,
    state: Arc<Mutex<RelayState>>,
    _subscriber_conn: Connection,
) -> Result<()> {
    let subscriber_writer = Arc::new(Mutex::new(subscriber_writer));

    // === Filter check ===
    // This minimal implementation only supports NextGroupStart.
    let has_unsupported_filter = msg.parameters.iter().any(|p| {
        matches!(
            p,
            MessageParameter::SubscriptionFilter(f)
            if !matches!(f, SubscriptionFilter::NextGroupStart)
        )
    });
    if has_unsupported_filter {
        let err = RequestErrorMessage {
            error_code: 0x3, // NOT_SUPPORTED
            retry_interval: 0,
            reason_phrase: ReasonPhrase {
                value: b"only NextGroupStart filter is supported".to_vec(),
            },
        };
        subscriber_writer
            .lock()
            .await
            .write_request_error(&err)
            .await?;
        return Ok(());
    }

    // === Find publisher ===
    // Prefix-match: registered ["example"] matches subscribe ["example", "live"].
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
                    .ok_or_else(|| anyhow::anyhow!("publisher session gone"))?
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
                subscriber_writer
                    .lock()
                    .await
                    .write_request_error(&err)
                    .await?;
                return Ok(());
            }
        }
    };

    // === Forward SUBSCRIBE to publisher ===
    // Assign a relay-owned request ID so the relay can manage multiple subscribers.
    let (pub_send, pub_recv) = publisher_conn.open_bi().await?;
    let mut pub_writer = RequestStreamWriter::new(pub_send);
    let mut pub_reader = RequestStreamReader::new(pub_recv);

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
    pub_writer.write_subscribe(&upstream_subscribe).await?;

    // === Receive and forward SUBSCRIBE_OK ===
    let pub_msg = pub_reader.read_message().await?;
    let subscribe_ok = match pub_msg {
        RequestMessage::SubscribeOk(ok) => ok,
        _ => bail!("expected SUBSCRIBE_OK from publisher"),
    };

    let track_alias = subscribe_ok.track_alias;

    // === Record subscription entry ===
    {
        let mut s = state.lock().await;
        s.subscriptions.push(SubscriptionEntry {
            subscriber_session,
            publisher_session: publisher_session_id,
            track_namespace: msg.track_namespace.clone(),
            track_name: msg.track_name.clone(),
            publisher_track_alias: track_alias,
            subscriber_track_alias: track_alias,
            subscriber_bidi_send: subscriber_writer.clone(),
        });
    }

    // Forward SUBSCRIBE_OK to subscriber
    subscriber_writer
        .lock()
        .await
        .write_subscribe_ok(&subscribe_ok)
        .await?;

    // === Wait for PUBLISH_DONE and forward ===
    let pub_msg = pub_reader.read_message().await?;
    let publish_done = match pub_msg {
        RequestMessage::PublishDone(done) => done,
        _ => bail!("expected PUBLISH_DONE from publisher"),
    };

    let subs_to_notify: Vec<Arc<Mutex<RequestStreamWriter>>> = {
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

    for writer in subs_to_notify {
        let _ = writer.lock().await.write_publish_done(&publish_done).await;
    }

    Ok(())
}

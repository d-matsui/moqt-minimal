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

use anyhow::Result;

use quinn::Endpoint;
use tokio::sync::Mutex;

use moqt_core::data::subgroup_header::SubgroupHeader;
use moqt_core::message::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::message::request_error::RequestErrorMessage;
use moqt_core::primitives::reason_phrase::ReasonPhrase;
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::data_stream::{DataStreamReader, DataStreamWriter};
use moqt_core::session::moqt_session::{MoqtSession, RequestEvent};
use moqt_core::session::subscribe_request::SubscribeRequest;

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
}

/// Per-session state. Holds a reference to the MOQT session.
struct SessionState {
    session: Arc<MoqtSession>,
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
    /// Subscriber's request handle.
    /// Used to forward PUBLISH_DONE.
    subscriber_bidi_send: Arc<Mutex<SubscribeRequest>>,
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

    // === SETUP exchange ===
    let session = Arc::new(MoqtSession::accept(connection).await?);

    // Assign a session ID and register in the session list
    let session_id = {
        let mut s = state.lock().await;
        let id = s.next_session_id;
        s.next_session_id += 1;
        s.sessions.insert(
            id,
            SessionState {
                session: session.clone(),
            },
        );
        id
    };

    // === Main loop: process requests and data streams concurrently ===
    loop {
        tokio::select! {
            request = session.next_request() => {
                match request {
                    Ok(event) => {
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_request_event(session_id, event, state).await {
                                eprintln!("request error: {e}");
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
            data = session.accept_data_stream() => {
                match data {
                    Ok((header, reader)) => {
                        let state = state.clone();
                        let sid = session_id;
                        tokio::spawn(async move {
                            if let Err(e) = handle_data_stream(sid, header, reader, state).await {
                                eprintln!("data stream error: {e}");
                            }
                        });
                    }
                    Err(_) => break,
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
/// 1. Identify the target subscription from the Track Alias in the SubgroupHeader
/// 2. Open new uni streams to all subscribers
/// 3. Forward the SubgroupHeader to subscribers
/// 4. Read objects one by one and immediately forward them to subscribers
///    (relay with low latency without buffering the entire stream)
/// 5. Propagate stream termination (FIN) to subscribers
async fn handle_data_stream(
    sender_session: SessionId,
    header: SubgroupHeader,
    mut data_reader: DataStreamReader,
    state: Arc<Mutex<RelayState>>,
) -> Result<()> {
    let track_alias = header.track_alias;

    // === Identify subscribers and open downstream streams ===
    // Find matching subscriptions by Track Alias and sender session,
    // then open uni streams to each subscriber
    let subscriber_writers: Vec<DataStreamWriter> = {
        let s = state.lock().await;
        let sub_conns: Vec<quinn::Connection> = s
            .subscriptions
            .iter()
            .filter(|sub| {
                sub.publisher_session == sender_session && sub.publisher_track_alias == track_alias
            })
            .filter_map(|sub| {
                s.sessions
                    .get(&sub.subscriber_session)
                    .map(|sess| sess.session.connection().clone())
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
    // Re-encode the header and write to subscribers
    let subscriber_writers: Vec<Arc<Mutex<DataStreamWriter>>> = subscriber_writers
        .into_iter()
        .map(|w| Arc::new(Mutex::new(w)))
        .collect();

    for writer in &subscriber_writers {
        writer.lock().await.write_subgroup_header(&header).await?;
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

/// Handle an incoming request event.
async fn handle_request_event(
    session_id: SessionId,
    event: RequestEvent,
    state: Arc<Mutex<RelayState>>,
) -> Result<()> {
    match event {
        RequestEvent::PublishNamespace(mut request) => {
            {
                let mut s = state.lock().await;
                s.namespace_publishers
                    .insert(request.message.track_namespace.clone(), session_id);
            }
            request.accept().await?;
        }
        RequestEvent::Subscribe(request) => {
            handle_subscribe(session_id, request, state).await?;
        }
    }

    Ok(())
}

/// Handle a SUBSCRIBE message.
///
/// 1. Check subscription filter (only NextGroupStart supported)
/// 2. Find publisher session by namespace (prefix match)
/// 3. Forward SUBSCRIBE to publisher via session API
/// 4. Record subscription entry (used for data stream relay)
/// 5. Forward SUBSCRIBE_OK to subscriber
/// 6. Wait for PUBLISH_DONE from publisher and forward to subscriber
async fn handle_subscribe(
    subscriber_session: SessionId,
    request: SubscribeRequest,
    state: Arc<Mutex<RelayState>>,
) -> Result<()> {
    let msg = request.message.clone();
    let subscriber_request = Arc::new(Mutex::new(request));

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
        subscriber_request.lock().await.reject(&err).await?;
        return Ok(());
    }

    // === Find publisher session ===
    // Prefix-match: registered ["example"] matches subscribe ["example", "live"].
    let (publisher_session_id, publisher_session) = {
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
                let session = s
                    .sessions
                    .get(&id)
                    .ok_or_else(|| anyhow::anyhow!("publisher session gone"))?
                    .session
                    .clone();
                (id, session)
            }
            None => {
                let err = RequestErrorMessage {
                    error_code: 0x10, // DOES_NOT_EXIST
                    retry_interval: 0,
                    reason_phrase: ReasonPhrase {
                        value: b"no publisher for namespace".to_vec(),
                    },
                };
                subscriber_request.lock().await.reject(&err).await?;
                return Ok(());
            }
        }
    };

    // === Forward SUBSCRIBE to publisher via session API ===
    let mut subscription = publisher_session
        .subscribe(
            msg.track_namespace.clone(),
            msg.track_name.clone(),
            msg.parameters.clone(),
        )
        .await?;

    let track_alias = subscription.track_alias();

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
            subscriber_bidi_send: subscriber_request.clone(),
        });
    }

    // Forward SUBSCRIBE_OK to subscriber
    subscriber_request
        .lock()
        .await
        .accept(&subscription.subscribe_ok)
        .await?;

    // === Wait for PUBLISH_DONE and forward ===
    let publish_done = subscription.recv_publish_done().await?;

    let subs_to_notify: Vec<Arc<Mutex<SubscribeRequest>>> = {
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

    for req in subs_to_notify {
        let _ = req.lock().await.send_publish_done(&publish_done).await;
    }

    Ok(())
}

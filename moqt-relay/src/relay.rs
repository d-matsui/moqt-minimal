//! # relay: MOQT relay server implementation
//!
//! This module implements the core logic of the MOQT relay.
//!
//! ## Architecture overview
//!
//! ```text
//! Publisher ──QUIC conn──→ [Relay Server] ←──QUIC conn── Subscriber
//!   │                              │                              │
//!   ├─ SETUP exchange              │               SETUP exchange─┤
//!   ├─ PUBLISH_NAMESPACE register  │                              │
//!   │                              │← SUBSCRIBE ──────────────────┤
//!   │← SUBSCRIBE forward ──────────┤                              │
//!   ├─ SUBSCRIBE_OK ──────────────→┤                              │
//!   │                              ├─ SUBSCRIBE_OK forward ──────→│
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
//! - `namespace_to_publisher`: namespace-to-publisher mapping
//! - `subscriptions`: list of active subscriptions

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use quinn::Endpoint;
use tokio::sync::Mutex;

use moqt_core::data::subgroup_header::SubgroupHeader;
use moqt_core::message::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::message::request_error::{ERROR_DOES_NOT_EXIST, ERROR_NOT_SUPPORTED};
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::data_stream::DataStreamReader;
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
    next_session_id: u64,
    sessions: HashMap<SessionId, Arc<MoqtSession>>,
    namespace_to_publisher: HashMap<TrackNamespace, SessionId>,
    subscriptions: Vec<SubscriptionEntry>,
}

impl RelayState {
    /// Register a new session and return its ID.
    fn register_session(&mut self, session: Arc<MoqtSession>) -> SessionId {
        let id = self.next_session_id;
        self.next_session_id += 1;
        self.sessions.insert(id, session);
        id
    }

    /// Remove a session and all associated namespace registrations and subscriptions.
    fn remove_session(&mut self, id: SessionId) {
        self.sessions.remove(&id);
        self.namespace_to_publisher.retain(|_, v| *v != id);
        self.subscriptions
            .retain(|sub| sub.subscriber_session != id && sub.publisher_session != id);
    }

    /// Register a namespace as published by the given session.
    fn register_namespace(&mut self, namespace: TrackNamespace, session_id: SessionId) {
        self.namespace_to_publisher.insert(namespace, session_id);
    }

    /// Find the publisher session for a namespace (prefix match).
    /// Find the publisher session for a namespace (prefix match).
    /// Tries progressively shorter prefixes of the given namespace
    /// against the HashMap until a match is found.
    fn find_publisher(&self, namespace: &TrackNamespace) -> Option<(SessionId, Arc<MoqtSession>)> {
        let mut prefix = namespace.clone();
        loop {
            if let Some(&pub_id) = self.namespace_to_publisher.get(&prefix) {
                let session = self.sessions.get(&pub_id)?.clone();
                return Some((pub_id, session));
            }
            if prefix.fields.is_empty() {
                return None;
            }
            prefix.fields.pop();
        }
    }

    /// Add a subscription entry.
    fn add_subscription(&mut self, entry: SubscriptionEntry) {
        self.subscriptions.push(entry);
    }

    /// Find subscriber sessions for a given publisher's data stream.
    fn find_subscriber_sessions(
        &self,
        publisher_session: SessionId,
        track_alias: u64,
    ) -> Vec<Arc<MoqtSession>> {
        self.subscriptions
            .iter()
            .filter(|sub| {
                sub.publisher_session == publisher_session
                    && sub.publisher_track_alias == track_alias
            })
            .filter_map(|sub| self.sessions.get(&sub.subscriber_session).cloned())
            .collect()
    }

    /// Find subscriber request handles for a given track (for PUBLISH_DONE forwarding).
    fn find_subscriber_requests(
        &self,
        publisher_session: SessionId,
        namespace: &TrackNamespace,
        track_name: &[u8],
    ) -> Vec<Arc<Mutex<SubscribeRequest>>> {
        self.subscriptions
            .iter()
            .filter(|sub| {
                sub.publisher_session == publisher_session
                    && sub.track_namespace == *namespace
                    && sub.track_name == track_name
            })
            .map(|sub| sub.subscriber_bidi_send.clone())
            .collect()
    }
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
                namespace_to_publisher: HashMap::new(),
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

    let session_id = state.lock().await.register_session(session.clone());

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
    state.lock().await.remove_session(session_id);

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
    let sub_sessions = state
        .lock()
        .await
        .find_subscriber_sessions(sender_session, track_alias);

    // If there are no subscribers, drain the stream
    if sub_sessions.is_empty() {
        while let Ok(Some(_)) = data_reader.read_object().await {}
        return Ok(());
    }

    // === Open data streams to subscribers (writes SubgroupHeader) ===
    let mut writers = Vec::new();
    for session in &sub_sessions {
        match session.open_data_stream(&header).await {
            Ok(w) => writers.push(Arc::new(Mutex::new(w))),
            Err(e) => eprintln!("failed to open data stream to subscriber: {e}"),
        }
    }
    let subscriber_writers = writers;

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
            state
                .lock()
                .await
                .register_namespace(request.message.track_namespace.clone(), session_id);
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
        subscriber_request
            .lock()
            .await
            .reject(ERROR_NOT_SUPPORTED, "only NextGroupStart filter is supported")
            .await?;
        return Ok(());
    }

    // === Find publisher session ===
    let (publisher_session_id, publisher_session) = match state
        .lock()
        .await
        .find_publisher(&msg.track_namespace)
    {
        Some(found) => found,
        None => {
            subscriber_request
                .lock()
                .await
                .reject(ERROR_DOES_NOT_EXIST, "no publisher for namespace")
                .await?;
            return Ok(());
        }
    };

    // === Forward SUBSCRIBE to publisher via session API ===
    let track_name_str = std::str::from_utf8(&msg.track_name)?;
    let mut subscription = publisher_session
        .subscribe(
            msg.track_namespace.clone(),
            track_name_str,
            msg.parameters.clone(),
        )
        .await?;

    let track_alias = subscription.track_alias();

    // === Record subscription entry ===
    state.lock().await.add_subscription(SubscriptionEntry {
        subscriber_session,
        publisher_session: publisher_session_id,
        track_namespace: msg.track_namespace.clone(),
        track_name: msg.track_name.clone(),
        publisher_track_alias: track_alias,
        subscriber_track_alias: track_alias,
        subscriber_bidi_send: subscriber_request.clone(),
    });

    // Forward SUBSCRIBE_OK to subscriber
    subscriber_request
        .lock()
        .await
        .accept(&subscription.subscribe_ok)
        .await?;

    // === Wait for PUBLISH_DONE and forward ===
    let publish_done = subscription.recv_publish_done().await?;

    let subs_to_notify = state.lock().await.find_subscriber_requests(
        publisher_session_id,
        &msg.track_namespace,
        &msg.track_name,
    );

    for req in subs_to_notify {
        let _ = req.lock().await.send_publish_done(&publish_done).await;
    }

    Ok(())
}

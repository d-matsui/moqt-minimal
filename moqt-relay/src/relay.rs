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

use moqt_core::message::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::message::request_error::{ERROR_DOES_NOT_EXIST, ERROR_NOT_SUPPORTED};
use moqt_core::message::subscribe_ok::SubscribeOkMessage;
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::group::GroupReader;
use moqt_core::session::moqt_session::{MoqtSession, SessionEvent};
use moqt_core::session::subscribe_request::SubscribeRequest;

/// Unique identifier for a session. Assigned sequentially per connection.
type SessionId = u64;

/// MOQT relay server. Holds a QUIC endpoint and accepts connections.
pub struct Relay {
    endpoint: Endpoint,
    /// State shared across all sessions. Protected by a Mutex.
    state: Arc<Mutex<RelayState>>,
}

/// A full track name that uniquely identifies a track (namespace + track name).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FullTrackName {
    namespace: TrackNamespace,
    name: String,
}

/// An active subscription for a track, from one publisher to one or more subscribers.
struct Subscription {
    /// Session ID of the publisher delivering data
    publisher_session: SessionId,
    /// Track alias assigned by the publisher (used in SubgroupHeader)
    publisher_track_alias: u64,
    /// The SUBSCRIBE_OK received from the publisher.
    /// Reused for subscription aggregation.
    subscribe_ok: SubscribeOkMessage,
    /// List of subscribers receiving data for this track
    subscribers: Vec<SubscriberEntry>,
}

/// A subscriber within a subscription.
struct SubscriberEntry {
    session_id: SessionId,
    /// Request handle for sending PUBLISH_DONE
    request: Arc<Mutex<SubscribeRequest>>,
}

/// Shared relay state. Manages sessions, namespace registrations, and subscriptions.
struct RelayState {
    next_session_id: u64,
    sessions: HashMap<SessionId, Arc<MoqtSession>>,
    namespace_to_publisher: HashMap<TrackNamespace, SessionId>,
    subscriptions: HashMap<FullTrackName, Subscription>,
    /// Per-track locks to serialize handle_subscribe for the same track.
    /// Prevents duplicate upstream SUBSCRIBEs when multiple subscribers
    /// request the same track concurrently.
    track_locks: HashMap<FullTrackName, Arc<Mutex<()>>>,
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
        // Remove subscriber entries; drop entire subscription if publisher disconnects
        // or no subscribers remain.
        self.subscriptions.retain(|_, sub| {
            if sub.publisher_session == id {
                return false;
            }
            sub.subscribers.retain(|s| s.session_id != id);
            !sub.subscribers.is_empty()
        });
    }

    /// Register a namespace as published by the given session.
    fn register_namespace(&mut self, namespace: TrackNamespace, session_id: SessionId) {
        self.namespace_to_publisher.insert(namespace, session_id);
    }

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

    /// Add a subscriber to an existing subscription, or create a new one.
    fn add_subscriber(
        &mut self,
        track: FullTrackName,
        publisher_session: SessionId,
        publisher_track_alias: u64,
        subscribe_ok: SubscribeOkMessage,
        subscriber: SubscriberEntry,
    ) {
        let sub = self
            .subscriptions
            .entry(track)
            .or_insert_with(|| Subscription {
                publisher_session,
                publisher_track_alias,
                subscribe_ok,
                subscribers: Vec::new(),
            });
        sub.subscribers.push(subscriber);
    }

    /// Find subscriber sessions for a given publisher's data stream.
    fn find_subscriber_sessions(
        &self,
        publisher_session: SessionId,
        track_alias: u64,
    ) -> Vec<Arc<MoqtSession>> {
        self.subscriptions
            .values()
            .filter(|sub| {
                sub.publisher_session == publisher_session
                    && sub.publisher_track_alias == track_alias
            })
            .flat_map(|sub| &sub.subscribers)
            .filter_map(|s| self.sessions.get(&s.session_id).cloned())
            .collect()
    }

    /// Find an existing subscription for the same track (for aggregation).
    fn find_existing_subscription(&self, track: &FullTrackName) -> Option<&Subscription> {
        self.subscriptions.get(track)
    }

    /// Find subscriber request handles for a given track (for PUBLISH_DONE forwarding).
    fn find_subscriber_requests(&self, track: &FullTrackName) -> Vec<Arc<Mutex<SubscribeRequest>>> {
        self.subscriptions
            .get(track)
            .map(|sub| sub.subscribers.iter().map(|s| s.request.clone()).collect())
            .unwrap_or_default()
    }
}

impl Relay {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            state: Arc::new(Mutex::new(RelayState {
                next_session_id: 0,
                sessions: HashMap::new(),
                namespace_to_publisher: HashMap::new(),
                subscriptions: HashMap::new(),
                track_locks: HashMap::new(),
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

    // === Main loop: handle events until the connection closes ===
    loop {
        let event = match session.next_event().await {
            Ok(event) => event,
            Err(_) => break, // connection closed
        };

        let state = state.clone();
        match event {
            SessionEvent::Subscribe(request) => {
                tokio::spawn(async move {
                    if let Err(e) = handle_subscribe(session_id, request, state).await {
                        eprintln!("subscribe error: {e}");
                    }
                });
            }
            SessionEvent::PublishNamespace(mut request) => {
                tokio::spawn(async move {
                    state
                        .lock()
                        .await
                        .register_namespace(request.message.track_namespace.clone(), session_id);
                    if let Err(e) = request.accept().await {
                        eprintln!("publish_namespace error: {e}");
                    }
                });
            }
            SessionEvent::DataStream(group_reader) => {
                tokio::spawn(async move {
                    if let Err(e) = handle_data_stream(session_id, group_reader, state).await {
                        eprintln!("data stream error: {e}");
                    }
                });
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
    mut group_reader: GroupReader,
    state: Arc<Mutex<RelayState>>,
) -> Result<()> {
    let track_alias = group_reader.track_alias();

    // === Identify subscribers and open downstream streams ===
    // Find matching subscriptions by Track Alias and sender session,
    // then open uni streams to each subscriber
    let sub_sessions = state
        .lock()
        .await
        .find_subscriber_sessions(sender_session, track_alias);

    // If there are no subscribers, drain the stream
    if sub_sessions.is_empty() {
        while let Ok(Some(_)) = group_reader.read_object_raw().await {}
        return Ok(());
    }

    // === Open data streams to subscribers (writes SubgroupHeader) ===
    let mut writers = Vec::new();
    for session in &sub_sessions {
        match session.open_data_stream(group_reader.header()).await {
            Ok(w) => writers.push(Arc::new(Mutex::new(w))),
            Err(e) => eprintln!("failed to open data stream to subscriber: {e}"),
        }
    }
    let subscriber_writers = writers;

    // === Relay objects incrementally ===
    // Read objects one by one and immediately forward to all subscribers.
    // No buffering, so memory usage stays low even for large streams.
    while let Some((_obj, payload, obj_header_bytes)) = group_reader.read_object_raw().await? {
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
            .reject(
                ERROR_NOT_SUPPORTED,
                "only NextGroupStart filter is supported",
            )
            .await?;
        return Ok(());
    }

    let track_name_str = std::str::from_utf8(&msg.track_name)?;
    let track = FullTrackName {
        namespace: msg.track_namespace.clone(),
        name: track_name_str.to_string(),
    };

    // === Per-track serialization ===
    // Acquire a per-track lock to prevent duplicate upstream SUBSCRIBEs
    // when multiple subscribers request the same track concurrently.
    let track_lock = state
        .lock()
        .await
        .track_locks
        .entry(track.clone())
        .or_default()
        .clone();
    let track_guard = track_lock.lock().await;

    let new_subscriber = SubscriberEntry {
        session_id: subscriber_session,
        request: subscriber_request.clone(),
    };

    // === Subscription aggregation ===
    // If there is already an upstream subscription for the same track,
    // reuse it instead of sending a new SUBSCRIBE to the publisher.
    {
        let mut s = state.lock().await;
        if let Some(existing) = s.find_existing_subscription(&track) {
            let ok = existing.subscribe_ok.clone();
            s.subscriptions
                .get_mut(&track)
                .unwrap()
                .subscribers
                .push(new_subscriber);
            drop(s);
            subscriber_request.lock().await.accept(&ok).await?;
            return Ok(());
        }
    }

    // === Find publisher session ===
    let (publisher_session_id, publisher_session) =
        match state.lock().await.find_publisher(&msg.track_namespace) {
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
    let mut subscription = publisher_session
        .subscribe(
            msg.track_namespace.clone(),
            track_name_str,
            msg.parameters.clone(),
        )
        .await?;

    let track_alias = subscription.track_alias();

    // === Record subscription ===
    state.lock().await.add_subscriber(
        track.clone(),
        publisher_session_id,
        track_alias,
        subscription.subscribe_ok.clone(),
        new_subscriber,
    );

    // Release the per-track lock now that the subscription is established.
    // Other subscribers for this track can now proceed with aggregation.
    drop(track_guard);

    // Forward SUBSCRIBE_OK to subscriber
    subscriber_request
        .lock()
        .await
        .accept(&subscription.subscribe_ok)
        .await?;

    // === Wait for PUBLISH_DONE and forward ===
    let publish_done = subscription.recv_publish_done().await?;

    let subs_to_notify = state.lock().await.find_subscriber_requests(&track);

    for req in subs_to_notify {
        let _ = req.lock().await.send_publish_done(&publish_done).await;
    }

    Ok(())
}

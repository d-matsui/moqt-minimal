//! # relay: MOQT リレーサーバーの実装
//!
//! このモジュールが MOQT リレーの中核ロジックを実装する。
//!
//! ## アーキテクチャ概要
//!
//! ```text
//! パブリッシャー ──QUIC接続──→ [リレーサーバー] ←──QUIC接続── サブスクライバー
//!   │                              │                              │
//!   ├─ SETUP 交換                  │                  SETUP 交換 ─┤
//!   ├─ PUBLISH_NAMESPACE 登録      │                              │
//!   │                              ├─ SUBSCRIBE 転送 ────────────→│
//!   │                 SUBSCRIBE_OK ←┤                              │
//!   ├─ データストリーム(uni) ─────→├─ データストリーム中継 ──────→│
//!   └─ PUBLISH_DONE ──────────────→├─ PUBLISH_DONE 転送 ────────→│
//! ```
//!
//! ## 接続ごとの処理フロー
//! 1. 新しい QUIC 接続を受け入れ、セッション ID を割り当てる
//! 2. SETUP メッセージを交換する
//! 3. 双方向ストリーム（bidi）で制御メッセージを処理:
//!    - PUBLISH_NAMESPACE: 名前空間を登録し REQUEST_OK を返す
//!    - SUBSCRIBE: パブリッシャーに転送し、応答をサブスクライバーに返す
//! 4. 単方向ストリーム（uni）でデータを中継:
//!    - SubgroupHeader の Track Alias から購読を特定
//!    - オブジェクトを1つずつ読みながらサブスクライバーに転送
//!
//! ## 共有状態（RelayState）
//! 全セッション間で共有される状態を `Arc<Mutex<RelayState>>` で管理する。
//! - `sessions`: 接続中のセッション一覧
//! - `namespace_publishers`: 名前空間→パブリッシャーのマッピング
//! - `subscriptions`: アクティブな購読一覧

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};

use quinn::{Connection, Endpoint};
use tokio::sync::Mutex;

use moqt_core::message::parameter::{MessageParameter, SubscriptionFilter};
use moqt_core::message::request_error::RequestErrorMessage;
use moqt_core::message::request_ok::RequestOkMessage;
use moqt_core::message::setup::SetupMessage;
use moqt_core::message::subscribe::SubscribeMessage;
use moqt_core::primitives::reason_phrase::ReasonPhrase;
use moqt_core::primitives::track_namespace::TrackNamespace;
use moqt_core::session::control_stream::{ControlStreamReader, ControlStreamWriter};
use moqt_core::session::data_stream::{DataStreamReader, DataStreamWriter};
use moqt_core::session::request_id::RequestIdAllocator;
use moqt_core::session::request_stream::{
    RequestMessage, RequestStreamReader, RequestStreamWriter,
};

/// セッションの一意な識別子。接続ごとに連番で割り当てる。
type SessionId = u64;

/// MOQT リレーサーバー。QUIC エンドポイントを持ち、接続を受け付ける。
pub struct Relay {
    endpoint: Endpoint,
    /// 全セッション間で共有される状態。Mutex で排他制御する。
    state: Arc<Mutex<RelayState>>,
}

/// リレーの共有状態。セッション管理、名前空間登録、購読管理を行う。
struct RelayState {
    /// 次に割り当てるセッション ID
    next_session_id: u64,
    /// アクティブなセッションの一覧
    sessions: HashMap<SessionId, SessionState>,
    /// 名前空間→パブリッシャーセッション ID のマッピング。
    /// サブスクライバーの SUBSCRIBE を適切なパブリッシャーに転送するために使う。
    namespace_publishers: HashMap<TrackNamespace, SessionId>,
    /// アクティブな購読一覧。データストリームの中継先の特定に使う。
    subscriptions: Vec<SubscriptionEntry>,
    /// サーバー側のリクエスト ID アロケータ（奇数 ID を生成）。
    /// パブリッシャーへの SUBSCRIBE 転送時に新しい ID を割り当てる。
    request_id_alloc: RequestIdAllocator,
}

/// 個々のセッションの状態。QUIC 接続への参照を保持する。
struct SessionState {
    connection: Connection,
}

/// 購読エントリ。サブスクライバーとパブリッシャーの対応関係を記録する。
struct SubscriptionEntry {
    /// 購読を要求したサブスクライバーのセッション ID
    subscriber_session: SessionId,
    /// データを配信するパブリッシャーのセッション ID
    publisher_session: SessionId,
    /// 購読対象のトラック名前空間
    track_namespace: TrackNamespace,
    /// 購読対象のトラック名
    track_name: Vec<u8>,
    /// パブリッシャーが割り当てたトラックエイリアス。
    /// データストリームの SubgroupHeader に含まれるので、
    /// この値でどの購読に対するデータかを特定する。
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

    /// リレーサーバーのメインループ。
    /// 新しい QUIC 接続を受け付け、各接続を非同期タスクで処理する。
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

/// 1つの QUIC 接続（セッション）を処理する。
/// SETUP 交換後、bidi ストリームと uni ストリームを並行して処理する。
async fn handle_connection(incoming: quinn::Incoming, state: Arc<Mutex<RelayState>>) -> Result<()> {
    let connection = incoming.await?;

    // セッション ID を割り当て、セッション一覧に登録
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
    // Server -> Client: send empty SETUP
    let our_ctrl_send = connection.open_uni().await?;
    let mut ctrl_writer = ControlStreamWriter::new(our_ctrl_send);
    let server_setup = SetupMessage {
        setup_options: vec![],
    };
    ctrl_writer.write_setup(&server_setup).await?;

    // クライアントからサーバーへ: SETUP を受信
    let peer_ctrl_recv = connection.accept_uni().await?;
    let mut ctrl_reader = ControlStreamReader::new(peer_ctrl_recv);
    let _peer_setup = ctrl_reader.read_setup().await?;

    // === メインループ: bidi ストリームと uni ストリームを並行処理 ===
    // tokio::select! で両方を同時に待ち受け、先に到着した方を処理する。
    // bidi: 制御メッセージ（PUBLISH_NAMESPACE, SUBSCRIBE）
    // uni: データストリーム（SubgroupHeader + Objects）
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

    // === 切断時のクリーンアップ ===
    // このセッションに関連する全ての状態を削除する。
    // 名前空間登録と購読エントリを除去しないと、
    // 切断済みセッションへの転送が試みられてしまう。
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

/// パブリッシャーからの単方向データストリームを処理し、サブスクライバーに中継する。
///
/// ## 中継の流れ
/// 1. SubgroupHeader を読み取り、Track Alias から対象の購読を特定
/// 2. 全サブスクライバーに対して新しい uni ストリームを開く
/// 3. SubgroupHeader をサブスクライバーに転送
/// 4. オブジェクトを1つずつ読みながら即座にサブスクライバーに転送
///    （ストリーム全体をバッファリングせず、低遅延で中継する）
/// 5. ストリーム終了（FIN）をサブスクライバーに伝搬
async fn handle_data_stream(
    sender_session: SessionId,
    recv: quinn::RecvStream,
    state: Arc<Mutex<RelayState>>,
) -> Result<()> {
    let mut data_reader = DataStreamReader::new(recv);

    // === SubgroupHeader の読み取りと検証 ===
    let (header, header_bytes) = data_reader.read_subgroup_header().await?;
    let track_alias = header.track_alias;

    // === サブスクライバーの特定と下流ストリームの開設 ===
    // Track Alias と送信元セッションから対象の購読を見つけ、
    // 各サブスクライバーへの uni ストリームを開く
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
        // ロックを解放してからストリームを開く（await を含むため）
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

    // サブスクライバーがいなければ、ストリームを読み捨てる
    if subscriber_writers.is_empty() {
        while let Ok(Some(_)) = data_reader.read_object().await {}
        return Ok(());
    }

    // === SubgroupHeader の転送 ===
    // 読み取った生バイト列をそのままサブスクライバーに書き込む
    let subscriber_writers: Vec<Arc<Mutex<DataStreamWriter>>> = subscriber_writers
        .into_iter()
        .map(|w| Arc::new(Mutex::new(w)))
        .collect();

    for writer in &subscriber_writers {
        writer.lock().await.write_raw(&header_bytes).await?;
    }

    // === オブジェクトの逐次中継 ===
    // オブジェクトを1つずつ読み、即座に全サブスクライバーに転送する。
    // バッファリングしないため、大きなストリームでもメモリ使用量が抑えられる。
    while let Some((_obj, payload, obj_header_bytes)) = data_reader.read_object().await? {
        // 全サブスクライバーに即座に転送
        // エラーが発生しても他のサブスクライバーへの転送は継続する
        for writer in &subscriber_writers {
            let mut w = writer.lock().await;
            let _ = w.write_raw(&obj_header_bytes).await;
            let _ = w.write_raw(&payload).await;
        }
    }

    // === ストリーム終了の伝搬 ===
    // パブリッシャーのストリームが終了したら、
    // サブスクライバーのストリームも finish() で FIN を送る
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

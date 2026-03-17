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
    /// サブスクライバーへの bidi ストリーム送信側。
    /// PUBLISH_DONE の転送に使う。
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

    // === SETUP 交換 ===
    // サーバーからクライアントへ: 空の SETUP を送信
    let mut our_ctrl_send = connection.open_uni().await?;
    let server_setup = SetupMessage {
        setup_options: vec![],
    };
    let mut setup_buf = Vec::new();
    server_setup.encode(&mut setup_buf)?;
    our_ctrl_send.write_all(&setup_buf).await?;

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
                            if let Err(e) = handle_bidi_stream(session_id, send, recv, state, conn).await {
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
                            if let Err(e) = handle_incoming_uni(sid, recv, state).await {
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

/// QUIC RecvStream から正確に n バイトを読み取る。
/// quinn の read() は要求より少ないバイト数を返すことがあるため、
/// 必要量に達するまでループする。
async fn read_exact(recv: &mut quinn::RecvStream, n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    let mut filled = 0;
    while filled < n {
        match recv.read(&mut buf[filled..]).await {
            Ok(Some(read)) => filled += read,
            Ok(None) => {
                bail!("stream ended after {filled}/{n} bytes");
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(buf)
}

/// QUIC RecvStream から varint を1つ読み取る。
/// (デコードされた値, 生のバイト列) を返す。
///
/// control_stream.rs の read_varint と同じロジックだが、
/// こちらは quinn::RecvStream を直接扱う（ControlStreamReader を経由しない）。
/// データストリームの中継時に使われる。
async fn read_varint_from_stream(recv: &mut quinn::RecvStream) -> Result<(u64, Vec<u8>)> {
    let first = read_exact(recv, 1).await?;
    let byte = first[0];
    // 先頭バイトのプレフィックスビットから総バイト数を決定
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
        bail!("invalid varint code point 0xFC");
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

/// パブリッシャーからの単方向データストリームを処理し、サブスクライバーに中継する。
///
/// ## 中継の流れ
/// 1. SubgroupHeader を読み取り、Track Alias から対象の購読を特定
/// 2. 全サブスクライバーに対して新しい uni ストリームを開く
/// 3. SubgroupHeader をサブスクライバーに転送
/// 4. オブジェクトを1つずつ読みながら即座にサブスクライバーに転送
///    （ストリーム全体をバッファリングせず、低遅延で中継する）
/// 5. ストリーム終了（FIN）をサブスクライバーに伝搬
async fn handle_incoming_uni(
    sender_session: SessionId,
    mut recv: quinn::RecvStream,
    state: Arc<Mutex<RelayState>>,
) -> Result<()> {
    // === SubgroupHeader の読み取り ===
    // Type (varint) + Track Alias (varint) + Group ID (varint) を個別に読む
    let (stream_type, type_bytes) = read_varint_from_stream(&mut recv).await?;
    let (track_alias, alias_bytes) = read_varint_from_stream(&mut recv).await?;
    let (_group_id, group_bytes) = read_varint_from_stream(&mut recv).await?;

    // ビット4がセットされていれば Subgroup Header（仕様による）
    if stream_type & 0x10 == 0 {
        bail!("expected SUBGROUP_HEADER, got type 0x{stream_type:X}");
    }

    // === サブスクライバーの特定と下流ストリームの開設 ===
    // Track Alias と送信元セッションから対象の購読を見つけ、
    // 各サブスクライバーへの uni ストリームを開く
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
        // ロックを解放してからストリームを開く（await を含むため）
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

    // サブスクライバーがいなければ、ストリームを読み捨てる
    if subscriber_streams.is_empty() {
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

    // === SubgroupHeader の転送 ===
    // 読み取った生バイト列をそのままサブスクライバーに書き込む
    let mut header_bytes = Vec::new();
    header_bytes.extend_from_slice(&type_bytes);
    header_bytes.extend_from_slice(&alias_bytes);
    header_bytes.extend_from_slice(&group_bytes);

    let subscriber_streams: Vec<Arc<Mutex<quinn::SendStream>>> = subscriber_streams
        .into_iter()
        .map(|s| Arc::new(Mutex::new(s)))
        .collect();

    for stream in &subscriber_streams {
        stream.lock().await.write_all(&header_bytes).await?;
    }

    // === オブジェクトの逐次中継 ===
    // オブジェクトを1つずつ読み、即座に全サブスクライバーに転送する。
    // バッファリングしないため、大きなストリームでもメモリ使用量が抑えられる。
    loop {
        // Object ID Delta (varint) を読む
        let delta_result = read_varint_from_stream(&mut recv).await;
        let (_delta, delta_bytes) = match delta_result {
            Ok(v) => v,
            Err(e) => {
                // ストリーム FIN または読み取りエラー → ストリーム終了として扱う
                let is_eof = e.downcast_ref::<quinn::ReadExactError>().is_some()
                    || e.to_string().contains("stream ended");
                if is_eof {
                    break;
                }
                return Err(e);
            }
        };

        // Payload Length (varint) を読む
        let (payload_len, len_bytes) = read_varint_from_stream(&mut recv).await?;

        // ペイロード本体を読む
        let payload = read_exact(&mut recv, payload_len as usize).await?;

        // 全サブスクライバーに即座に転送
        // エラーが発生しても他のサブスクライバーへの転送は継続する
        for stream in &subscriber_streams {
            let mut s = stream.lock().await;
            let _ = s.write_all(&delta_bytes).await;
            let _ = s.write_all(&len_bytes).await;
            let _ = s.write_all(&payload).await;
        }
    }

    // === ストリーム終了の伝搬 ===
    // パブリッシャーのストリームが終了したら、
    // サブスクライバーのストリームも finish() で FIN を送る
    for stream in subscriber_streams {
        let mut s = stream.lock().await;
        let _ = s.finish();
    }

    Ok(())
}

/// 双方向ストリーム上の制御メッセージを処理する。
/// メッセージタイプを読み取り、PUBLISH_NAMESPACE か SUBSCRIBE に分岐する。
async fn handle_bidi_stream(
    session_id: SessionId,
    mut send: quinn::SendStream,
    recv: quinn::RecvStream,
    state: Arc<Mutex<RelayState>>,
    connection: Connection,
) -> Result<()> {
    let mut reader = ControlStreamReader::new(recv);
    let msg_bytes = reader.read_message_bytes().await?;

    // メッセージタイプを先読みして分岐を決定
    let mut slice = msg_bytes.as_slice();
    let msg_type = decode_varint(&mut slice)?;

    // デコード用にスライスをリセット（先頭から再度デコードするため）
    let mut slice = msg_bytes.as_slice();

    match msg_type {
        MSG_PUBLISH_NAMESPACE => {
            // パブリッシャーが名前空間を登録
            let msg = PublishNamespaceMessage::decode(&mut slice)?;
            {
                let mut s = state.lock().await;
                s.namespace_publishers
                    .insert(msg.track_namespace.clone(), session_id);
            }
            // 登録成功を応答
            let ok = RequestOkMessage {};
            let mut buf = Vec::new();
            ok.encode(&mut buf);
            send.write_all(&buf).await?;
        }
        MSG_SUBSCRIBE => {
            // サブスクライバーがトラックを購読
            let msg = SubscribeMessage::decode(&mut slice)?;
            handle_subscribe(session_id, msg, send, state, connection).await?;
        }
        _ => {
            bail!("unexpected message type on bidi stream: 0x{msg_type:X}");
        }
    }

    Ok(())
}

/// SUBSCRIBE メッセージを処理する。
///
/// ## 処理フロー
/// 1. 名前空間からパブリッシャーを検索（前方一致で検索）
/// 2. パブリッシャーが見つからなければ REQUEST_ERROR を返す
/// 3. パブリッシャーに SUBSCRIBE を転送（新しいリクエスト ID を割り当て）
/// 4. パブリッシャーから SUBSCRIBE_OK を受信
/// 5. 購読エントリを記録（データストリームの中継に使用）
/// 6. サブスクライバーに SUBSCRIBE_OK を転送
/// 7. パブリッシャーから PUBLISH_DONE を待ち、サブスクライバーに転送
async fn handle_subscribe(
    subscriber_session: SessionId,
    msg: SubscribeMessage,
    subscriber_send: quinn::SendStream,
    state: Arc<Mutex<RelayState>>,
    _subscriber_conn: Connection,
) -> Result<()> {
    let subscriber_send = Arc::new(Mutex::new(subscriber_send));

    // === パブリッシャーの検索 ===
    // 登録された名前空間の中から、SUBSCRIBE の名前空間に前方一致するものを探す。
    // 例: パブリッシャーが ["example"] を登録し、サブスクライバーが ["example", "live"] を
    // 購読した場合、["example"] が ["example", "live"] の前方一致となるのでマッチする。
    let (publisher_session_id, publisher_conn) = {
        let s = state.lock().await;
        let ns = &msg.track_namespace;
        let pub_id = s
            .namespace_publishers
            .iter()
            .find_map(|(registered_ns, sid)| {
                // 登録名前空間のフィールド数が購読名前空間以下で、
                // 先頭のフィールドが全て一致すれば前方一致とみなす
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
                // パブリッシャーが見つからない → エラー応答
                let err = RequestErrorMessage {
                    error_code: 0x10, // DOES_NOT_EXIST
                    retry_interval: 0,
                    reason_phrase: ReasonPhrase {
                        value: b"no publisher for namespace".to_vec(),
                    },
                };
                let mut buf = Vec::new();
                err.encode(&mut buf);
                subscriber_send.lock().await.write_all(&buf).await?;
                return Ok(());
            }
        }
    };

    // === SUBSCRIBE をパブリッシャーに転送 ===
    // リレー独自のリクエスト ID を割り当てて転送する。
    // サブスクライバーの ID をそのまま使わないのは、
    // リレーが複数のサブスクライバーからの SUBSCRIBE を管理するため。
    let (mut pub_send, pub_recv) = publisher_conn.open_bi().await?;

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
    upstream_subscribe.encode(&mut buf)?;
    pub_send.write_all(&buf).await?;

    // === SUBSCRIBE_OK の受信と転送 ===
    let mut pub_reader = ControlStreamReader::new(pub_recv);
    let ok_bytes = pub_reader.read_message_bytes().await?;
    let mut ok_slice = ok_bytes.as_slice();
    let subscribe_ok = SubscribeOkMessage::decode(&mut ok_slice)?;

    let track_alias = subscribe_ok.track_alias;

    // === 購読エントリの記録 ===
    // この情報は、データストリーム中継時にパブリッシャーの Track Alias から
    // サブスクライバーを特定するために使われる。
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

    // サブスクライバーに SUBSCRIBE_OK を転送
    let mut ok_buf = Vec::new();
    subscribe_ok.encode(&mut ok_buf);
    subscriber_send.lock().await.write_all(&ok_buf).await?;

    // === PUBLISH_DONE の待機と転送 ===
    // パブリッシャーが配信を終了するまでこのタスクは生き続ける。
    // PUBLISH_DONE を受信したら、同じトラックを購読している
    // 全サブスクライバーに転送する。
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

# 設計: moqt-relay

```
moqt-relay/src/
├── main.rs
└── relay.rs
```

## relay.rs の責務

1. QUIC server として listen
2. 接続ごとに SETUP 交換
3. PUBLISH_NAMESPACE を受けたら、該当 namespace の Publisher としてセッションを記録
4. SUBSCRIBE を受けたら、namespace が一致する Publisher セッションに転送
5. Publisher からの Object stream を受信 → 該当トラックを SUBSCRIBE している全セッションに転送
6. Track Alias の管理（変換テーブルを持つ）

## 状態管理

```rust
struct RelayState {
    // 全セッション
    sessions: HashMap<SessionId, SessionState>,

    // namespace → Publisher セッションの対応（PUBLISH_NAMESPACE で登録）
    namespace_publishers: HashMap<TrackNamespace, SessionId>,

    // トラックごとの購読関係
    subscriptions: Vec<SubscriptionMapping>,
}

struct SessionState {
    connection: quinn::Connection,
    control_stream_writer: ControlStreamWriter,
    control_stream_reader: ControlStreamReader,
}

struct FullTrackName {
    namespace: TrackNamespace,
    name: Vec<u8>,
}

struct SubscriptionMapping {
    subscriber_session: SessionId,
    publisher_session: SessionId,
    track: FullTrackName,
    subscriber_track_alias: u64,  // Relay → Subscriber 間で使う Alias
    publisher_track_alias: u64,   // Publisher → Relay 間で使う Alias
}
```

## セッションと role の関係

仕様上、1つのセッションで Publisher と Subscriber の両方の役割を持てる。
role はセッション全体の属性ではなく、トラックごとに決まる:

- SUBSCRIBE を送信した側 → そのトラックに対して Subscriber
- SUBSCRIBE_OK を返して Object を送信する側 → そのトラックに対して Publisher

したがって Relay は接続時に role を判定する必要はない。
SETUP 交換後、bidi stream 上のメッセージ（SUBSCRIBE）の到着によってトラック単位で役割が決まる。

## Relay の動作

1. 任意のセッションから PUBLISH_NAMESPACE を受信 → 該当 namespace の Publisher として記録
2. 任意のセッションから SUBSCRIBE を受信 → namespace が一致する PUBLISH_NAMESPACE 済みセッションに転送
3. 該当 namespace の PUBLISH_NAMESPACE がなければ REQUEST_ERROR を返す

## Object stream の転送方式

Relay は **Object 単位でストリーミング転送**する（stream 全体をバッファしてから転送しない）:

1. Publisher の uni stream から SUBGROUP_HEADER を読む
2. 該当する全 Subscriber に対して uni stream を開き、SUBGROUP_HEADER を書き込む
3. Publisher の stream から Object header + Payload を1つ読むたびに、即座に全 Subscriber の stream に書き込む
4. Publisher の stream が FIN で閉じたら、Subscriber の stream も FIN で閉じる

これにより低遅延のリアルタイム転送が実現できる。

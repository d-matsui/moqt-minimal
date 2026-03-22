# 設計: moqt-relay

```
moqt-relay/src/
├── main.rs    # サーバ起動、自己署名証明書生成
└── relay.rs   # Relay 本体（状態管理 + イベントループ）
```

## relay.rs の責務

1. QUIC server として listen（ALPN: moqt-17 + h3 の両方を受付）
2. 接続ごとに ALPN を判定し、raw QUIC または WebTransport ハンドシェイクを実行
3. MOQT SETUP 交換
4. PUBLISH_NAMESPACE を受けたら、該当 namespace の Publisher としてセッションを記録
5. SUBSCRIBE を受けたら、namespace が一致する Publisher セッションに転送
6. Publisher からの Object stream を受信 → 該当トラックを SUBSCRIBE している全セッションに転送
7. Track Alias の管理（Publisher 側と Subscriber 側で独立）

## 状態管理

```rust
struct RelayState {
    sessions: HashMap<SessionId, Arc<MoqtSession>>,
    namespace_to_publisher: HashMap<TrackNamespace, SessionId>,
    subscriptions: HashMap<FullTrackName, Subscription>,
    track_locks: HashMap<FullTrackName, Arc<Mutex<()>>>,
}

struct Subscription {
    publisher_session: SessionId,
    publisher_track_alias: u64,
    subscribe_ok: SubscribeOkMessage,
    subscribers: Vec<SubscriberEntry>,
}
```

## セッションと role の関係

仕様上、1つのセッションで Publisher と Subscriber の両方の役割を持てる。
role はセッション全体の属性ではなく、トラックごとに決まる:

- SUBSCRIBE を送信した側 → そのトラックに対して Subscriber
- SUBSCRIBE_OK を返して Object を送信する側 → そのトラックに対して Publisher

SETUP 交換後、bidi stream 上のメッセージの到着によってトラック単位で役割が決まる。

## Subscription Aggregation

同一トラックに対する複数の SUBSCRIBE は1つに集約する:

1. 2人目以降の SUBSCRIBE 到着時、既存の upstream subscription を確認
2. 存在すれば Publisher への SUBSCRIBE は送らず、subscriber リストに追加
3. 既存の SUBSCRIBE_OK を Subscriber に転送

per-track ロックで同時に同じトラックへの SUBSCRIBE が競合しないようにする。

## Object stream の転送方式

Relay は **Object 単位でストリーミング転送**する（stream 全体をバッファしない）:

1. Publisher の uni stream から SubgroupHeader を読む
2. 該当する全 Subscriber に対して uni stream を開き、SubgroupHeader を書き込む
3. Publisher の stream から Object header + Payload を1つ読むたびに、即座に全 Subscriber の stream に書き込む
4. 書き込みエラーが発生した Subscriber はリストから除外する
5. Publisher の stream が FIN で閉じたら、Subscriber の stream も FIN で閉じる

## Cross-transport 転送

raw QUIC と WebTransport はどちらも `web_transport_quinn::Session` に統一されるため、
転送コードはトランスポートの違いを意識しない。
例: raw QUIC publisher → relay → WebTransport subscriber が追加コードなしで動作する。

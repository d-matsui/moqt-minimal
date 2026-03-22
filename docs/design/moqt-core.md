# 設計: moqt-core

Publisher / Relay / Subscriber が共通で使うライブラリ。

## レイヤー構成

```
wire/     → encode/decode（純粋関数、I/O なし）
stream/   → ストリーム上のフレーミング（async I/O）
session/  → プロトコルロジック + 公開 API
```

依存方向: `session/ → stream/ → wire/`

## モジュール構成

```
moqt-core/src/
├── lib.rs
├── client.rs              # connect() ヘルパー（QUIC 接続 + SETUP 一括）
├── quic_config.rs         # QUIC/TLS 設定（ALPN: moqt-17 + h3）
│
├── wire/                  # ワイヤフォーマットの encode/decode（全14ファイル）
│   ├── mod.rs             # メッセージタイプ定数, encode/decode_message
│   ├── varint.rs          # Variable-Length Integer (vi64)
│   ├── track_namespace.rs # Track Namespace
│   ├── key_value_pair.rs  # Key-Value-Pair (Setup Options 用)
│   ├── reason_phrase.rs   # Reason Phrase
│   ├── parameter.rs       # Message Parameters, SubscriptionFilter
│   ├── setup.rs           # SETUP
│   ├── subscribe.rs       # SUBSCRIBE
│   ├── subscribe_ok.rs    # SUBSCRIBE_OK
│   ├── publish_namespace.rs # PUBLISH_NAMESPACE
│   ├── request_ok.rs      # REQUEST_OK
│   ├── request_error.rs   # REQUEST_ERROR
│   ├── publish_done.rs    # PUBLISH_DONE
│   ├── subgroup_header.rs # SubgroupHeader
│   └── object.rs          # ObjectHeader
│
├── stream/                # QUIC ストリーム上のフレーミング
│   ├── mod.rs             # read_varint, read_message_frame（共通関数）
│   ├── control.rs         # ControlStreamReader/Writer（SETUP 用 uni stream）
│   ├── request.rs         # RequestStreamReader/Writer + RequestMessage enum
│   └── data.rs            # DataStreamReader/Writer（SubgroupHeader + Objects）
│
└── session/               # プロトコルロジックと公開 API
    ├── mod.rs             # MoqtSession, SessionEvent, RequestIdAllocator
    ├── subgroup.rs        # SubgroupReader/Writer（ObjectHeader を隠蔽）
    ├── subscribe_request.rs     # 着信 SUBSCRIBE の accept/reject
    ├── publish_namespace_request.rs  # 着信 PUBLISH_NAMESPACE の accept/reject
    └── subscription.rs    # 確立済みサブスクリプション（PUBLISH_DONE 受信）
```

## トランスポート

`stream/` と `session/` は `web_transport_quinn` の型を直接使用する:

- `web_transport_quinn::SendStream` / `RecvStream` — ストリーム I/O
- `web_transport_quinn::Session` — 接続（open_uni, accept_bi 等）

raw QUIC 接続は `Session::raw(quinn::Connection)` でラップされるため、
WebTransport 接続と同じ型で扱える。

## wire モジュール

最下層。バイト列の読み書きのみ。I/O やセッションの知識なし。
プリミティブ型、制御メッセージ、データストリームヘッダが全てフラットに並ぶ。

各型に `encode(&self, buf: &mut Vec<u8>)` と `decode(buf: &mut &[u8]) -> Result<Self>` を実装。

## stream モジュール

wire 型の知識 + QUIC ストリーム I/O を組み合わせる層。
セッション状態は知らない。

- `read_varint`: ストリームから varint を1つ非同期で読む（1バイト目で長さ判定）
- `read_message_frame`: メッセージフレーム（Type + Length + Payload）を1個読む
- `ControlStreamReader/Writer`: SETUP の読み書き
- `RequestStreamReader/Writer`: 制御メッセージの読み書き + RequestMessage ディスパッチ
- `DataStreamReader/Writer`: SubgroupHeader + Object の読み書き

## session モジュール

プロトコルロジックの最上層。外部 crate（pub, sub, relay）が使う公開 API。

- `MoqtSession`: SETUP ハンドシェイク、イベントディスパッチ、ストリーム操作
- `SessionEvent`: 着信イベント（Subscribe, PublishNamespace, DataStream）
- `SubgroupReader/Writer`: Object 単位の読み書き（ObjectHeader を隠蔽）
- `SubscribeRequest`: 着信 SUBSCRIBE の accept/reject + PUBLISH_DONE 送信
- `Subscription`: 確立済みサブスクリプション（track_alias, PUBLISH_DONE 受信）

MoqtSession は control stream を保持し、セッション終了まで FIN を送らない（Section 3.3）。

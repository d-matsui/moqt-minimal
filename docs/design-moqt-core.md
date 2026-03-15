# 設計: moqt-core

Publisher / Relay / Subscriber が共通で使うライブラリ。

## モジュール構成

```
moqt-core/src/
├── lib.rs
├── wire/                  # ワイヤフォーマットのエンコード/デコード
│   ├── mod.rs
│   ├── varint.rs          # Variable-Length Integer (vi64)
│   ├── track_namespace.rs # Track Namespace のエンコード/デコード
│   ├── reason_phrase.rs   # Reason Phrase のエンコード/デコード
│   └── key_value_pair.rs  # Key-Value-Pair (Setup Options 用)
├── message/               # 制御メッセージの定義とシリアライズ/パース
│   ├── mod.rs
│   ├── setup.rs           # SETUP
│   ├── publish_namespace.rs # PUBLISH_NAMESPACE
│   ├── request_ok.rs      # REQUEST_OK
│   ├── subscribe.rs       # SUBSCRIBE
│   ├── subscribe_ok.rs    # SUBSCRIBE_OK
│   ├── request_error.rs   # REQUEST_ERROR
│   └── publish_done.rs    # PUBLISH_DONE
├── data/                  # データストリームの定義
│   ├── mod.rs
│   ├── subgroup_header.rs # SUBGROUP_HEADER
│   └── object.rs          # Object fields (Object ID Delta, Payload Length, Payload)
└── session/               # セッション管理の共通ロジック
    ├── mod.rs
    ├── control_stream.rs  # Control stream の読み書き
    └── request_id.rs      # Request ID の採番
```

## wire モジュール

最下層。バイト列の読み書きのみ。QUIC やセッションの知識なし。

```rust
// varint.rs
pub fn encode_varint(value: u64, buf: &mut Vec<u8>);
pub fn decode_varint(buf: &mut &[u8]) -> Result<u64, DecodeError>;

// track_namespace.rs
pub struct TrackNamespace {
    pub fields: Vec<Vec<u8>>,
}

// reason_phrase.rs
pub struct ReasonPhrase {
    pub value: Vec<u8>,  // UTF-8
}
```

## message モジュール

制御メッセージの型定義とシリアライズ/パース。wire モジュールを使う。

```rust
// setup.rs
pub struct SetupMessage {
    pub setup_options: Vec<SetupOption>,
}

pub enum SetupOption {
    Path(Vec<u8>),
    Authority(Vec<u8>),
    Unknown { type_id: u64, value: Vec<u8> },
}

// publish_namespace.rs
pub struct PublishNamespaceMessage {
    pub request_id: u64,
    pub required_request_id_delta: u64,
    pub track_namespace: TrackNamespace,
    pub parameters: Vec<MessageParameter>,
}

// request_ok.rs
pub struct RequestOkMessage {
    pub parameters: Vec<MessageParameter>,
}

// subscribe.rs
pub struct SubscribeMessage {
    pub request_id: u64,
    pub required_request_id_delta: u64,
    pub track_namespace: TrackNamespace,
    pub track_name: Vec<u8>,
    pub parameters: Vec<MessageParameter>,
}

// subscribe_ok.rs
pub struct SubscribeOkMessage {
    pub track_alias: u64,
    pub parameters: Vec<MessageParameter>,
    pub track_properties: Vec<u8>,  // 最小実装では空
}

// request_error.rs
pub struct RequestErrorMessage {
    pub error_code: u64,
    pub retry_interval: u64,
    pub reason_phrase: ReasonPhrase,
}

// publish_done.rs
pub struct PublishDoneMessage {
    pub status_code: u64,
    pub stream_count: u64,
    pub reason_phrase: ReasonPhrase,
}
```

各メッセージに `encode(&self, buf: &mut Vec<u8>)` と `decode(buf: &mut &[u8]) -> Result<Self, DecodeError>` を実装。

## data モジュール

データストリームのヘッダーとオブジェクトフィールドの型定義。

```rust
// subgroup_header.rs
pub struct SubgroupHeader {
    pub track_alias: u64,
    pub group_id: u64,
    // Subgroup ID = 0 固定 (Type 0x38 の SUBGROUP_ID_MODE = 0b00)
    // Publisher Priority = デフォルト (Type 0x38 の DEFAULT_PRIORITY = 1)
}

// object.rs
pub struct ObjectHeader {
    pub object_id_delta: u64,
    pub payload_length: u64,
}
```

## session モジュール

QUIC ストリーム上でのメッセージ読み書きの共通ロジック。

```rust
// control_stream.rs
// Unidirectional stream 上で制御メッセージを送受信するヘルパー
pub struct ControlStreamWriter { /* quinn::SendStream をラップ */ }
pub struct ControlStreamReader { /* quinn::RecvStream をラップ */ }

// request_id.rs
pub struct RequestIdAllocator {
    next_id: u64,  // client: 0, 2, 4, ... / server: 1, 3, 5, ...
}
```

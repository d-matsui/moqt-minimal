# 設計: 全体構成

技術スタック: Rust + quinn (QUIC) + web-transport-quinn (WebTransport)

## クレート構成

```
moqt-core/          # MOQT プロトコルの共通ロジック（ライブラリ）
moqt-relay/          # Relay バイナリ
moqt-pub/            # Publisher バイナリ（テスト/デモ用）
moqt-sub/            # Subscriber バイナリ（テスト/デモ用）
```

## トランスポート

Relay は同一ポートで 2 種類のトランスポートを受け付ける:

| トランスポート | ALPN | 接続フロー |
|--------------|------|-----------|
| raw QUIC | `moqt-17` | QUIC handshake → MOQT SETUP |
| WebTransport | `h3` | QUIC handshake → HTTP/3 CONNECT → MOQT SETUP |

どちらも `web_transport_quinn::Session` に統一される（raw QUIC は `Session::raw()` でラップ）。
cross-transport 転送（例: QUIC publisher → WebTransport subscriber）も動作する。

## moqt-pub

1. Relay に接続（raw QUIC）
2. SETUP 交換
3. PUBLISH_NAMESPACE を送信し、REQUEST_OK を受信
4. SUBSCRIBE を受信したら SUBSCRIBE_OK を返す
5. `--pipe` モード: stdin から IVF/VP8 ストリームを読み、Object として送信
   - デフォルトモード: 疑似データ（固定バイト列）を送信
6. 終了時に PUBLISH_DONE を送信

### IVF/VP8 → MOQT マッピング（--pipe モード）

- VP8 キーフレーム（bit 0 = 0）→ 新しい Group を開始
- 各フレーム → 1 Object
- Group の先頭からデコード可能（キーフレームから始まる）

## moqt-sub

1. Relay に接続（raw QUIC）
2. SETUP 交換
3. SUBSCRIBE を送信
4. SUBSCRIBE_OK を受信
5. Object stream を受信し、payload を stdout に出力（または検証）

## メッセージフロー

### セッション確立

```
Peer                         Relay
   |                          |
   |---QUIC connect---------->|
   |                          |
   |---uni stream (ctrl)----->|   ← Peer の control stream
   |   SETUP                  |
   |                          |
   |<---uni stream (ctrl)-----|   ← Relay の control stream
   |   SETUP                  |
   |                          |
   | (セッション確立完了)       |
   | (この時点では role 未定)   |
```

Publisher / Subscriber どちらも同じ手順。role はその後のメッセージで決まる。

### Namespace 宣言 → 購読確立 → Object 転送

```
Publisher              Relay                Subscriber
   |                     |                     |
   |---bidi stream------>|                     |
   | PUBLISH_NAMESPACE   |                     |
   |<---REQUEST_OK-------|                     |
   |                     |                     |
   |                     |<---bidi stream------|
   |                     |   SUBSCRIBE         |
   |<---bidi stream------|                     |
   |   SUBSCRIBE         |                     |
   |                     |                     |
   |---SUBSCRIBE_OK----->|                     |
   | (track_alias=1)     |---SUBSCRIBE_OK----->|
   |                     | (track_alias=1)     |
   |                     |                     |
   |---uni stream------->|---uni stream------->|
   | SUBGROUP_HEADER     | SUBGROUP_HEADER     |
   | {alias=1, group=0}  | {alias=1, group=0}  |
   | obj(id=0, payload)  | obj(id=0, payload)  |
   | obj(id=1, payload)  | obj(id=1, payload)  |
   | FIN                 | FIN                 |
```

## QUIC ストリームの使い方まとめ

| 用途 | ストリーム種別 | 開く側 | 備考 |
|------|-------------|-------|------|
| Control stream | Unidirectional | 各 peer が1本ずつ | SETUP を先頭に。セッション中閉じてはならない |
| PUBLISH_NAMESPACE | Bidirectional | Publisher | PUBLISH_NAMESPACE → REQUEST_OK |
| SUBSCRIBE リクエスト | Bidirectional | Subscriber | SUBSCRIBE → SUBSCRIBE_OK/REQUEST_ERROR → PUBLISH_DONE |
| Object 送信 | Unidirectional | Publisher | SUBGROUP_HEADER + Objects、Subgroup ごとに新しい stream |

## テスト構成

### 単体テスト（各モジュール内）

各ソースファイル内に `#[cfg(test)] mod tests` として記述。

```
moqt-core/src/wire/varint.rs          → vi64 のテスト
moqt-core/src/wire/track_namespace.rs → Track Namespace のテスト
moqt-core/src/wire/reason_phrase.rs   → Reason Phrase のテスト
moqt-core/src/wire/key_value_pair.rs  → Key-Value-Pair のテスト
moqt-core/src/wire/setup.rs           → SETUP のテスト
moqt-core/src/wire/subscribe.rs       → SUBSCRIBE のテスト
moqt-core/src/wire/subscribe_ok.rs    → SUBSCRIBE_OK のテスト
moqt-core/src/wire/publish_namespace.rs → PUBLISH_NAMESPACE のテスト
moqt-core/src/wire/request_ok.rs      → REQUEST_OK のテスト
moqt-core/src/wire/request_error.rs   → REQUEST_ERROR のテスト
moqt-core/src/wire/publish_done.rs    → PUBLISH_DONE のテスト
moqt-core/src/wire/subgroup_header.rs → SUBGROUP_HEADER のテスト
moqt-core/src/wire/object.rs          → Object fields のテスト
```

### 結合テスト

実際に QUIC / WebTransport 接続を行うテスト。
Publisher / Relay / Subscriber を同一プロセス内で起動し、localhost で接続する。

```
moqt-relay/tests/integration.rs  → raw QUIC + WebTransport + cross-transport テスト
```

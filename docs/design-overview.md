# 設計: 全体構成

技術スタック: Rust + quinn (QUIC)

## クレート構成

```
moqt-core/          # MOQT プロトコルの共通ロジック（ライブラリ）
moqt-relay/          # Relay バイナリ
moqt-pub/            # Publisher バイナリ（テスト/デモ用）
moqt-sub/            # Subscriber バイナリ（テスト/デモ用）
```

## moqt-pub

1. Relay に QUIC 接続
2. SETUP 交換
3. PUBLISH_NAMESPACE を送信し、REQUEST_OK を受信
4. SUBSCRIBE を受信したら SUBSCRIBE_OK を返す
5. `--pipe` モード: stdin から H.264 Annex B バイトストリームを読み、Object として送信
   - デフォルトモード: 疑似データ（固定バイト列）を送信
6. 終了時に PUBLISH_DONE を送信

### H.264 → MOQT マッピング（--pipe モード）

前提: ffmpeg `-tune zerolatency` で SPS/PPS がキーフレームの前にインバンドで毎回送られる。

- 1 NAL unit = 1 Object
- SPS (NAL type 7) の到着で新しい Group を開始
  - SPS → PPS → IDR → P, P, P... が 1 Group にまとまる
  - Group の先頭からデコード開始可能（SPS + PPS + IDR が揃うため）
- SPS がインバンドで来ない設定の場合、この方式は使えない（IDR で Group を切り、SPS/PPS を別途渡す仕組みが必要）

## moqt-sub

1. Relay に QUIC 接続
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
| Control stream | Unidirectional | 各 peer が1本ずつ | SETUP を先頭に、以降は使わない（最小実装） |
| PUBLISH_NAMESPACE | Bidirectional | Publisher | PUBLISH_NAMESPACE → REQUEST_OK |
| SUBSCRIBE リクエスト | Bidirectional | Subscriber | SUBSCRIBE → SUBSCRIBE_OK/REQUEST_ERROR → PUBLISH_DONE |
| Object 送信 | Unidirectional | Publisher | SUBGROUP_HEADER + Objects、Group ごとに新しい stream |

## テスト構成

受け入れ要件（acceptance-criteria.md）のテストケースを以下の場所に配置する。

### 単体テスト（各モジュール内）

受け入れ要件 1〜2 に対応。各ソースファイル内に `#[cfg(test)] mod tests` として記述。

```
moqt-core/src/wire/varint.rs          → 1.1 vi64 のテスト
moqt-core/src/wire/track_namespace.rs → 1.2 Track Namespace のテスト
moqt-core/src/wire/reason_phrase.rs   → 1.3 Reason Phrase のテスト
moqt-core/src/wire/key_value_pair.rs  → 1.4 Key-Value-Pair のテスト
moqt-core/src/message/setup.rs        → 2.1 SETUP のテスト
moqt-core/src/message/subscribe.rs    → 2.2 SUBSCRIBE のテスト
moqt-core/src/message/subscribe_ok.rs → 2.3 SUBSCRIBE_OK のテスト
moqt-core/src/message/publish_namespace.rs → 2.4 PUBLISH_NAMESPACE のテスト
moqt-core/src/message/request_ok.rs   → 2.5 REQUEST_OK のテスト
moqt-core/src/message/request_error.rs → 2.6 REQUEST_ERROR のテスト
moqt-core/src/message/publish_done.rs → 2.7 PUBLISH_DONE のテスト
moqt-core/src/data/subgroup_header.rs → SUBGROUP_HEADER のテスト
moqt-core/src/data/object.rs          → Object fields のテスト
```

### 結合テスト（workspace ルート）

受け入れ要件 3〜7 に対応。実際に QUIC 接続を行うテスト。
Publisher / Relay / Subscriber を同一プロセス内で起動し、localhost で接続する。

```
moqt-relay/tests/integration.rs  → 3〜7 の結合テスト全て
```

# 最小MOQT実装 機能要件 (draft-ietf-moq-transport-17ベース)

## 目的

draft-17の仕様から、映像・音声のライブ配信に必要な最小限の機能を抽出する。
Publisher → Relay → Subscriber の1対N配信を最小構成で実現する。

## 構成

```
Publisher(s) --raw QUIC--> Relay <--raw QUIC-- Subscriber(s)
```

- トランスポート: raw QUIC（ALPN: `moqt-17`）
- Relay が QUIC server、Publisher/Subscriber が QUIC client
- QUIC DATAGRAM extension のネゴシエーションは必須（RFC 9221）。ただし実際の Object 送信は stream のみで可

## 1. データモデル

仕様 Section 2 から必要な概念のみ。

### Track

- namespace（ordered tuple of bytes）+ name（bytes）で一意に識別
- 映像・音声それぞれ別 Track とする
- 例: namespace=("example"), name="video" / name="audio"

### Group

- Track 内の時間的な区切り。Subscriber の join ポイント
- 映像: キーフレーム単位で新しい Group を開始
- 音声: 一定間隔（例: 1秒）で新しい Group を開始
- Group ID は 0 から単調増加

### Object

- Group 内の個々のデータ単位
- 映像: 1フレーム = 1 Object
- 音声: 1チャンク = 1 Object
- Object ID は Group 内で 0 から単調増加
- Object の内容（payload）は不変。一度送ったら変えない

### Subgroup

- Group 内で Object をまとめる単位。1 Subgroup = 1 QUIC unidirectional stream
- 最小実装では 1 Group につき 1 Subgroup（Subgroup ID = 0 固定）
- temporal scalability 等のレイヤー分離は行わない

## 2. セッション

仕様 Section 3 から。

### 確立

- Relay が QUIC server として listen
- Publisher/Subscriber が QUIC client として接続
- ALPN: `moqt-17`

### 初期化（Section 3.3）

- 各 peer が unidirectional stream を1本ずつ開き、SETUP メッセージを送信（計2本の control stream）
- SETUP 交換完了後にセッション確立

### Control stream

- セッション中、各 peer の control stream は閉じてはならない
- control stream を閉じた場合はプロトコル違反としてセッション終了

### Request stream

- SUBSCRIBE 等のリクエストは bidirectional stream 上で行う
- 1 リクエスト = 1 bidi stream

### 終了

- CONNECTION_CLOSE（QUIC）で終了
- エラーコード: NO_ERROR (0x0), PROTOCOL_VIOLATION (0x3) のみ最低限サポート

## 3. 制御メッセージ

仕様 Section 9 から。各メッセージの共通フォーマット:

```
Control Message {
  Message Type (vi64),
  Message Length (16),    // payload のバイト数
  Message Payload (..),
}
```

### 3.1 SETUP (0x2F00) — 必須

Section 9.4。セッション初期化。各 peer が control stream の先頭で送信。

```
SETUP Message {
  Type (vi64) = 0x2F00,
  Length (16),
  Setup Options (..) ...,
}
```

#### 最小実装の Setup Options

- PATH (0x01): client が送信。raw QUIC では MUST。最小実装では `"/"` 固定
- AUTHORITY (0x05): client が送信。raw QUIC では MUST。接続先ホスト名を指定
- 上記2つは仕様上送受信が必須だが、最小実装の Relay では受け取っても特に使わない（単一サービスのため分岐不要）
- 未知の Setup Option は無視する（MUST）

### 3.2 SUBSCRIBE (0x3) — 必須

Section 9.8。Subscriber がトラック購読を要求。新しい bidi stream 上で送信。

```
SUBSCRIBE Message {
  Type (vi64) = 0x3,
  Length (16),
  Request ID (vi64),
  Required Request ID Delta (vi64),    // 最小実装: 0（依存なし）
  Track Namespace (..),
  Track Name Length (vi64),
  Track Name (..),
  Number of Parameters (vi64),
  Parameters (..) ...
}
```

#### 最小実装の Parameters

- SUBSCRIPTION_FILTER (0x21): length-prefixed。NextGroupStart (0x1) のみ対応
  - ワイヤフォーマット: Parameter Type Delta (vi64) + Length (vi64) + Filter Type (vi64)
  - NextGroupStart の Filter Type = 0x1。optional fields なし
  - ライブ配信で次の Group（= キーフレーム境界）から受信開始する
- FORWARD (0x10): 1（省略時のデフォルト）
- その他は省略

#### Request ID の採番

- Client（Publisher/Subscriber）: 偶数（0, 2, 4, ...）
- Server（Relay）: 奇数（1, 3, 5, ...）

### 3.3 SUBSCRIBE_OK (0x4) — 必須

Section 9.9。Publisher が購読を承認。SUBSCRIBE と同じ bidi stream 上で応答。

```
SUBSCRIBE_OK Message {
  Type (vi64) = 0x4,
  Length (16),
  Track Alias (vi64),
  Number of Parameters (vi64),
  Parameters (..) ...,
  Track Properties (..),
}
```

- Track Alias: Object 送信時に Track を識別する数値。セッション内で一意
- Parameters:
  - LARGEST_OBJECT (0x9): Location 型（Group vi64 + Object vi64）。既に Object がある場合はその Location を返す
- Track Properties: 最小実装では空（長さ0）

### 3.4 PUBLISH_NAMESPACE (0x6) — 必須

Section 9.17。Publisher が Relay に対して「この namespace のトラックを持っている」と宣言する。新しい bidi stream 上で送信。

```
PUBLISH_NAMESPACE Message {
  Type (vi64) = 0x6,
  Length (16),
  Request ID (vi64),
  Required Request ID Delta (vi64),    // 最小実装: 0
  Track Namespace (..),
  Number of Parameters (vi64),         // 最小実装: 0
  Parameters (..) ...
}
```

Relay は PUBLISH_NAMESPACE を受信したセッションを、該当 namespace の Publisher として記録する。
Relay は REQUEST_OK または REQUEST_ERROR を同じ bidi stream 上で応答する。

### 3.5 REQUEST_OK (0x7) — 必須

Section 9.6。PUBLISH_NAMESPACE に対する成功応答。

```
REQUEST_OK Message {
  Type (vi64) = 0x7,
  Length (16),
  Number of Parameters (vi64),    // 最小実装: 0
  Parameters (..) ...
}
```

### 3.6 REQUEST_ERROR (0x5) — 必須

Section 9.7。リクエスト失敗時の応答。

```
REQUEST_ERROR Message {
  Type (vi64) = 0x5,
  Length (16),
  Error Code (vi64),
  Retry Interval (vi64),     // 最小リトライ待ち時間(ms) + 1。0 = リトライ不可、1 = 即時リトライ可
  Error Reason (Reason Phrase),
}
```

最小実装のエラーコード:
- INTERNAL_ERROR (0x0): 汎用エラー
- DOES_NOT_EXIST (0x10): 該当トラック/namespaceなし

### 3.7 PUBLISH_DONE (0xB) — 必須

Section 9.13。Publisher が配信終了を通知。SUBSCRIBE の bidi stream 上で送信。

```
PUBLISH_DONE Message {
  Type (vi64) = 0xB,
  Length (16),
  Status Code (vi64),
  Stream Count (vi64),
  Error Reason (Reason Phrase),
}
```

最小実装の Status Code:
- TRACK_ENDED (0x2): 配信終了

## 4. データ送信

仕様 Section 10 から。

### 4.1 Subgroup stream（unidirectional stream）

Publisher が Object を送信する唯一の方法（最小実装）。

#### SUBGROUP_HEADER

```
SUBGROUP_HEADER {
  Type (i),                    // 下記参照
  Track Alias (vi64),
  Group ID (vi64),
  [Subgroup ID (vi64),]        // Type の SUBGROUP_ID_MODE による
  [Publisher Priority (8),]    // Type の DEFAULT_PRIORITY bit による
}
```

最小実装の Type 値: `0x38` (`0b00111000`)

- bit 0 (PROPERTIES): 0 — Object Properties なし
- bit 1-2 (SUBGROUP_ID_MODE): 00 — Subgroup ID 省略（= 0）
- bit 3 (END_OF_GROUP): 1 — この subgroup が Group の最後の Object を含む（1 Group = 1 Subgroup なので常にセット）
- bit 4: 1（固定、Subgroup Header の識別）
- bit 5 (DEFAULT_PRIORITY): 1 — Priority 省略（デフォルト使用）

最小実装の SUBGROUP_HEADER:
```
SUBGROUP_HEADER {
  Type = 0x38,
  Track Alias (vi64),
  Group ID (vi64),
  // Subgroup ID 省略（= 0）
  // Publisher Priority 省略（デフォルト）
}
```

#### Object の送信

各 Object は SUBGROUP_HEADER に続けて以下を繰り返す:

```
{
  Object ID Delta (vi64),     // Object ID = 前の Object ID + Delta + 1。最初の Object では Object ID = Delta。連続 ID (0,1,2,...) なら全 Delta = 0
  Object Payload Length (vi64),
  Object Payload (..),
}
```

- Object Properties は省略（SUBGROUP_HEADER の PROPERTIES bit = 0）
- Object Status は省略（Payload Length > 0 なら暗黙的に Normal）

#### Stream のクローズ

- Group 内の全 Object を送信し終えたら FIN でストリームを閉じる
- 途中で中断する場合は RESET_STREAM

### 4.2 Datagram — 省略

最小実装では使用しない。全 Object を Subgroup stream で送信する。

## 5. ワイヤフォーマットの実装要素

### 5.1 Variable-Length Integer (vi64)

仕様 Section 1.4.1。全メッセージで使用。

| 先頭ビット | バイト数 | 有効ビット | 最大値 |
|-----------|---------|-----------|-------|
| 0         | 1       | 7         | 127 |
| 10        | 2       | 14        | 16383 |
| 110       | 3       | 21        | 2097151 |
| 1110      | 4       | 28        | 268435455 |

7バイト形式は存在しない（6バイトの次は8バイト）。`0xFC` (11111100) は無効コードポイント。
本実装では 9 バイトまで完全対応済み。

### 5.2 Track Namespace

```
Track Namespace {
  Number of Fields (vi64),
  Field {
    Field Length (vi64),
    Field Value (..)
  } ...
}
```

### 5.3 Reason Phrase

```
Reason Phrase {
  Reason Phrase Length (vi64),
  Reason Phrase Value (..)      // UTF-8
}
```

### 5.4 Key-Value-Pair（Setup Options 用）

```
Key-Value-Pair {
  Delta Type (vi64),
  [Length (vi64),]    // Type が奇数の場合のみ
  Value (..)
}
```

注意: Key-Value-Pair は Setup Options (Section 9.4) 用。Message Parameter (Section 9.3) は
別のエンコーディング（Type Delta + パラメータ定義ごとの Value 形式: uint8, varint, Location, length-prefixed）。
最小実装では必要な Message Parameter のみ個別に実装する。

## 6. Relay の動作

### 6.1 セッション管理

- QUIC server として listen
- 接続を受け入れ、各接続で SETUP を交換
- 接続時点では role を決定しない（トラック単位でメッセージ駆動で決まる）

### 6.2 PUBLISH_NAMESPACE の処理

1. Publisher が Relay に PUBLISH_NAMESPACE を送信
2. Relay が該当 namespace の Publisher としてセッションを記録
3. Relay が REQUEST_OK を返す

### 6.3 SUBSCRIBE の中継

1. Subscriber が Relay に SUBSCRIBE を送信
2. Relay が Track Namespace を見て、PUBLISH_NAMESPACE 済みのセッション（Publisher）を特定
3. Relay が Publisher に SUBSCRIBE を転送
   - Request ID は Relay-Publisher 間で独立に採番
4. Publisher が SUBSCRIBE_OK を返す
5. Relay が Subscriber に SUBSCRIBE_OK を返す
   - Track Alias は変換テーブルで管理

### 6.3 Object の転送

仕様 Section 8.4: "Each new Object belonging to the Track is forwarded to each subscriber"

Relay は stream 全体をバッファしてから転送するのではなく、**Object 単位で即座に転送する**:

1. Publisher から Subgroup stream（unidirectional）を受け取る
2. SUBGROUP_HEADER を読み、Track Alias から該当 Subscriber を特定する
3. 各 Subscriber に対して新しい Subgroup stream を開き、SUBGROUP_HEADER を書き込む
4. Object header（Object ID Delta + Payload Length）を読む
5. Payload を読む
6. **即座に** 各 Subscriber の stream に Object header + Payload を書き込む
7. 4-6 を stream が閉じるまで繰り返す
8. Publisher の stream が FIN で閉じたら、Subscriber の stream も FIN で閉じる

注意:
- Track Alias は SUBGROUP_HEADER に含まれる（Object fields には含まれない）。Subscriber 側の Alias に差し替えて書き込む
- Object fields（Object ID Delta, Payload Length, Payload）はそのまま転送（Section 8.7: "A relay MUST NOT modify Object fields"）
- キャッシュしない（pass-through）

### 6.4 PUBLISH_DONE の転送

1. Publisher が Relay に PUBLISH_DONE を送信
2. Relay が全 Subscriber に PUBLISH_DONE を転送

### 6.5 状態管理

```
Relay の内部状態:

Sessions:
  session_id → { connection, control_stream }

Namespace Publishers (PUBLISH_NAMESPACE で登録):
  namespace → { session_id }

Subscriptions:
  downstream_sub → {
    subscriber_session_id,
    upstream_sub (publisher 側の対応する subscription),
    downstream_track_alias,
    upstream_track_alias,
  }
```

## 7. 実装しないもの（明示的な除外リスト）

| 機能 | 仕様箇所 | 除外理由 |
|------|---------|---------|
| FETCH / FETCH_OK | Section 9.14-9.15 | 過去データ取得不要（ライブのみ） |
| PUBLISH / PUBLISH_OK | Section 9.11-9.12 | Publisher 起点の subscription は不要。SUBSCRIBE 方式のみ |
| REQUEST_UPDATE / REQUEST_OK | Section 9.10, 9.6 | 購読パラメータの動的変更不要 |
| GOAWAY | Section 9.5 | graceful migration 不要 |
| SUBSCRIBE_NAMESPACE | Section 9.20 | 同上 |
| NAMESPACE / NAMESPACE_DONE | Section 9.18-9.19 | 同上 |
| TRACK_STATUS | Section 9.16 | トラック状態照会不要 |
| PUBLISH_BLOCKED | Section 9.21 | 不要 |
| Datagram 送信 | Section 10.3 | stream のみで実装 |
| Subscription Filter (LatestObject, AbsoluteStart, AbsoluteRange) | Section 5.1.2 | NextGroupStart のみ対応 |
| Priority 制御 | Section 7 | デフォルト値（128）固定 |
| Authorization Token | Section 9.3.2 | 認証なし |
| Delivery Timeout | Section 9.3.3 | タイムアウト制御なし |
| RENDEZVOUS_TIMEOUT | Section 9.3.4 | 不要 |
| Properties (Track/Object) | Section 2.5, 11 | 空で送信 |
| キャッシュ | Section 8.1 | Relay はキャッシュしない |
| Subscription Aggregation | Section 8.4 | 各 Subscriber に個別に upstream SUBSCRIBE |
| Object dedup | Section 8.3 | しない |
| Group Order 選択 | Section 9.3.6 | Ascending 固定 |
| RESET_STREAM_AT | Section 10.4.3 | RESET_STREAM のみ使用 |

## 8. メッセージフロー

### 配信開始〜Object送信

```
Publisher              Relay                Subscriber
    |                    |                      |
    |---QUIC connect---->|<----QUIC connect------|
    |                    |                      |
    |---ctrl stream----->|                      |
    |    SETUP           |                      |
    |<---ctrl stream-----|                      |
    |    SETUP           |                      |
    |                    |---ctrl stream------->|
    |                    |    SETUP             |
    |                    |<---ctrl stream-------|
    |                    |    SETUP             |
    |                    |                      |
    |---bidi stream----->|                      |
    |  PUBLISH_NAMESPACE |                      |
    |<---REQUEST_OK------|                      |
    |  (on same bidi)    |                      |
    |                    |                      |
    |                    |<---bidi stream-------|
    |                    |    SUBSCRIBE         |
    |<---bidi stream-----|                      |
    |    SUBSCRIBE       |                      |
    |                    |                      |
    |---SUBSCRIBE_OK---->|                      |
    |  (on same bidi)    |---SUBSCRIBE_OK------>|
    |                    |  (on same bidi)      |
    |                    |                      |
    |---uni stream------>|---uni stream-------->|
    |  SUBGROUP_HEADER   |  SUBGROUP_HEADER    |
    |  Object 0          |  Object 0           |
    |  Object 1          |  Object 1           |
    |  ...               |  ...                |
    |  FIN               |  FIN                |
    |                    |                      |
    |  (次の Group)       |                      |
    |---uni stream------>|---uni stream-------->|
    |  SUBGROUP_HEADER   |  SUBGROUP_HEADER    |
    |  ...               |  ...                |
```

### 配信終了

```
Publisher              Relay                Subscriber
    |                    |                      |
    |---PUBLISH_DONE---->|---PUBLISH_DONE------>|
    |  (on bidi stream)  |  (on bidi stream)   |
    |                    |                      |
    |---close bidi------>|---close bidi-------->|
```

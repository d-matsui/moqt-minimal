# 受け入れ要件 / テストケース

技術スタック: Rust + quinn

## 前提

本テストケースは最小MOQT実装（minimal-moqt-spec.md）の以下の設計判断を前提とする:

- 1 Group = 1 Subgroup（Subgroup ID = 0 固定）
- 1 Subgroup = 1 QUIC unidirectional stream（仕様 Section 2.2）
- したがって 1 Group = 1 QUIC unidirectional stream

## 方針

- 各テストは Publisher / Relay / Subscriber を同一プロセス内で起動し、localhost で QUIC 接続して検証する
- 単体テスト: moqt-core/src/ 内 `#[cfg(test)]`
- 結合テスト: moqt-relay/tests/integration.rs

---

## 1. ワイヤフォーマット

### 1.1 Variable-Length Integer (vi64)

- [x] 1バイト値（0〜127）を正しくエンコード/デコードできる
- [x] 2バイト値（128〜16383）を正しくエンコード/デコードできる
- [x] 4バイト値（大きい値）を正しくエンコード/デコードできる
- [x] 仕様の例示値（37, 15293, 494878333）でラウンドトリップが一致する
- [x] 最小バイト数でエンコードされる（SHOULD）

### 1.2 Track Namespace

- [x] フィールド数0のnamespaceをエンコード/デコードできる
- [x] フィールド数1（例: "example"）をエンコード/デコードできる
- [x] 複数フィールド（例: "example", "live"）をエンコード/デコードできる

### 1.3 Reason Phrase

- [x] 空の reason phrase をエンコード/デコードできる
- [x] UTF-8文字列の reason phrase をエンコード/デコードできる

### 1.4 Key-Value-Pair (Setup Options)

- [x] 偶数Type（varint value）を正しくエンコード/デコードできる
- [x] 奇数Type（length-prefixed value）を正しくエンコード/デコードできる
- [x] Delta Type が正しく計算される（前のTypeとの差分）

---

## 2. 制御メッセージ

### 2.1 SETUP

- [x] client の SETUP（PATH, AUTHORITY 付き）をシリアライズ/パースできる
- [x] server の SETUP（Setup Options 空）をシリアライズ/パースできる
- [x] 未知の Setup Option を無視してパースが継続できる

### 2.2 SUBSCRIBE

- [x] Track Namespace + Track Name を含む SUBSCRIBE をシリアライズ/パースできる
- [x] Request ID が正しく設定される（client: 偶数, server: 奇数）
- [x] SUBSCRIPTION_FILTER パラメータ（NextGroupStart）を含められる
- [x] パラメータなし（Number of Parameters = 0）でも正しくパースできる

### 2.3 SUBSCRIBE_OK

- [x] Track Alias を含む SUBSCRIBE_OK をシリアライズ/パースできる
- [x] LARGEST_OBJECT パラメータを含められる
- [x] Track Properties が空でも正しくパースできる

### 2.4 PUBLISH_NAMESPACE

- [x] Track Namespace を含む PUBLISH_NAMESPACE をシリアライズ/パースできる
- [x] パラメータなし（Number of Parameters = 0）でも正しくパースできる

### 2.5 REQUEST_OK

- [x] パラメータなしの REQUEST_OK をシリアライズ/パースできる

### 2.6 REQUEST_ERROR

- [x] Error Code, Retry Interval, Reason Phrase を含む REQUEST_ERROR をシリアライズ/パースできる

### 2.7 PUBLISH_DONE

- [x] Status Code, Stream Count, Reason Phrase を含む PUBLISH_DONE をシリアライズ/パースできる

---

## 3. セッション確立

### 3.1 QUIC 接続

- [x] Relay が QUIC server として起動し、指定ポートで listen できる — `session_setup`
- [x] Publisher が Relay に QUIC 接続できる（ALPN: moqt-17）— `session_setup`
- [x] Subscriber が Relay に QUIC 接続できる（ALPN: moqt-17）— `subscribe_via_relay`
- [x] ALPN が一致しない場合、接続が拒否される — `alpn_mismatch`

### 3.2 SETUP 交換

- [x] Publisher ↔ Relay 間で SETUP を交換し、セッションが確立する — `session_setup`
- [x] Subscriber ↔ Relay 間で SETUP を交換し、セッションが確立する — `subscribe_via_relay`
- [x] 双方の control stream（unidirectional）が開かれる — `session_setup`

---

## 4. Namespace 宣言と購読フロー

### 4.1 PUBLISH_NAMESPACE → REQUEST_OK

- [x] Publisher が Relay に PUBLISH_NAMESPACE を送信し、Relay が REQUEST_OK を返す — `publish_namespace_registration`
- [x] Relay が該当 namespace の Publisher としてセッションを記録する — `subscribe_via_relay` で間接的に確認

### 4.2 SUBSCRIBE → SUBSCRIBE_OK

- [x] Publisher が PUBLISH_NAMESPACE 済みの状態で、Subscriber が Relay に SUBSCRIBE を送信する — `subscribe_via_relay`
- [x] Relay が namespace の一致する Publisher に SUBSCRIBE を転送する — `subscribe_via_relay`
- [x] Publisher が SUBSCRIBE_OK を返し、Relay 経由で Subscriber に届く — `subscribe_via_relay`
- [x] Relay が downstream の SUBSCRIBE_OK で Track Alias を返し、Object 転送時にその Alias が使われる — `object_forwarding`

### 4.3 SUBSCRIBE → REQUEST_ERROR

- [x] PUBLISH_NAMESPACE されていない namespace の Track を SUBSCRIBE した場合、REQUEST_ERROR が返る — `subscribe_unknown_namespace`

---

## 5. データ送信

### 5.1 Subgroup stream — 単一 Object

- [x] Publisher が 1 Group / 1 Object を送信し、Subscriber が受信できる — `object_forwarding`
- [x] SUBGROUP_HEADER の Track Alias で正しいトラックが識別される — `object_forwarding`
- [x] Object の payload が Publisher の送信内容と一致する — `object_forwarding`

### 5.2 Subgroup stream — 複数 Object

- [x] Publisher が 1 Group 内に複数 Object を送信し、Subscriber が全て順序通り受信できる — `object_forwarding`
- [x] Object ID Delta が正しくエンコード/デコードされる — `object_forwarding` + 単体テスト

### 5.3 複数 Group

- [x] Publisher が複数 Group を順次送信し、Subscriber が全て受信できる — `multiple_groups`
- [x] Group ごとに新しい unidirectional stream が開かれる（前提: 1 Group = 1 Subgroup = 1 stream）— `multiple_groups`
- [x] 各 Group の stream は全 Object 送信後に FIN で閉じられる — `multiple_groups`

### 5.4 複数 Track

- [x] Publisher が映像 Track と音声 Track を同時に配信できる — `multiple_tracks`
- [x] Subscriber が両方の Track を SUBSCRIBE し、両方の Object を受信できる — `multiple_tracks`
- [x] Track Alias によって Track が正しく識別される — `multiple_tracks`

---

## 6. Relay の動作

### 6.1 複数 Subscriber

- [x] 1 Publisher + 2 Subscriber の構成で、両方の Subscriber が同じ Object を受信できる — `multiple_subscribers`
- [x] Subscriber ごとに独立した Track Alias が割り当てられる — `multiple_subscribers`

### 6.2 PUBLISH_DONE の転送

- [x] Publisher が PUBLISH_DONE を送信し、Relay 経由で全 Subscriber に届く — `publish_done_forwarding`
- [x] Subscriber が PUBLISH_DONE を受信した後、購読が終了する — `publish_done_forwarding`

### 6.3 切断ハンドリング

- [ ] Publisher が切断した場合、Relay が Subscriber に PUBLISH_DONE（または接続切断）を通知する — **未テスト・未実装**
- [x] Subscriber が切断した場合、Relay が該当 Subscriber の購読状態を破棄する（Publisher 側の配信は継続する）— `subscriber_disconnect`

---

## 7. エンドツーエンド

### 7.1 基本シナリオ

- [x] Publisher がバイト列（疑似映像データ）を連続送信し、Subscriber が全て正しく受信できる — `multiple_groups`
- [x] 送信した payload と受信した payload がバイト単位で一致する — `object_forwarding`, `multiple_groups`

### 7.2 遅延参加

- [x] Publisher が配信中に Subscriber が接続した場合、次の Group の先頭から Object を受信できる（NextGroupStart フィルタ）— `late_join`

---

## 未達サマリ

| # | 項目 | 理由 |
|---|------|------|
| 6.3 | Publisher 切断時の Subscriber 通知 | 未テスト・未実装 |

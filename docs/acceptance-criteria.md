# 受け入れ要件 / テストケース

技術スタック: Rust + quinn

## 前提

本テストケースは最小MOQT実装（minimal-moqt-spec.md）の以下の設計判断を前提とする:

- 1 Group = 1 Subgroup（Subgroup ID = 0 固定）
- 1 Subgroup = 1 QUIC unidirectional stream（仕様 Section 2.2）
- したがって 1 Group = 1 QUIC unidirectional stream

## 方針

- 各テストは Publisher / Relay / Subscriber を同一プロセスまたは別プロセスで起動し、実際に QUIC 接続して検証する
- 単体テスト: ワイヤフォーマットのエンコード/デコード
- 結合テスト: メッセージフロー全体

---

## 1. ワイヤフォーマット

### 1.1 Variable-Length Integer (vi64)

- [ ] 1バイト値（0〜127）を正しくエンコード/デコードできる
- [ ] 2バイト値（128〜16383）を正しくエンコード/デコードできる
- [ ] 4バイト値（大きい値）を正しくエンコード/デコードできる
- [ ] 仕様の例示値（37, 15293, 494878333）でラウンドトリップが一致する
- [ ] 最小バイト数でエンコードされる（SHOULD）

### 1.2 Track Namespace

- [ ] フィールド数0のnamespaceをエンコード/デコードできる
- [ ] フィールド数1（例: "example"）をエンコード/デコードできる
- [ ] 複数フィールド（例: "example", "live"）をエンコード/デコードできる

### 1.3 Reason Phrase

- [ ] 空の reason phrase をエンコード/デコードできる
- [ ] UTF-8文字列の reason phrase をエンコード/デコードできる

### 1.4 Key-Value-Pair (Setup Options)

- [ ] 偶数Type（varint value）を正しくエンコード/デコードできる
- [ ] 奇数Type（length-prefixed value）を正しくエンコード/デコードできる
- [ ] Delta Type が正しく計算される（前のTypeとの差分）

---

## 2. 制御メッセージ

### 2.1 SETUP

- [ ] client の SETUP（PATH, AUTHORITY 付き）をシリアライズ/パースできる
- [ ] server の SETUP（Setup Options 空）をシリアライズ/パースできる
- [ ] 未知の Setup Option を無視してパースが継続できる

### 2.2 SUBSCRIBE

- [ ] Track Namespace + Track Name を含む SUBSCRIBE をシリアライズ/パースできる
- [ ] Request ID が正しく設定される（client: 偶数, server: 奇数）
- [ ] SUBSCRIPTION_FILTER パラメータ（NextGroupStart）を含められる
- [ ] パラメータなし（Number of Parameters = 0）でも正しくパースできる

### 2.3 SUBSCRIBE_OK

- [ ] Track Alias を含む SUBSCRIBE_OK をシリアライズ/パースできる
- [ ] LARGEST_OBJECT パラメータを含められる
- [ ] Track Properties が空でも正しくパースできる

### 2.4 PUBLISH_NAMESPACE

- [ ] Track Namespace を含む PUBLISH_NAMESPACE をシリアライズ/パースできる
- [ ] パラメータなし（Number of Parameters = 0）でも正しくパースできる

### 2.5 REQUEST_OK

- [ ] パラメータなしの REQUEST_OK をシリアライズ/パースできる

### 2.6 REQUEST_ERROR

- [ ] Error Code, Retry Interval, Reason Phrase を含む REQUEST_ERROR をシリアライズ/パースできる

### 2.7 PUBLISH_DONE

- [ ] Status Code, Stream Count, Reason Phrase を含む PUBLISH_DONE をシリアライズ/パースできる

---

## 3. セッション確立

### 3.1 QUIC 接続

- [ ] Relay が QUIC server として起動し、指定ポートで listen できる
- [ ] Publisher が Relay に QUIC 接続できる（ALPN: moqt-17）
- [ ] Subscriber が Relay に QUIC 接続できる（ALPN: moqt-17）
- [ ] ALPN が一致しない場合、接続が拒否される

### 3.2 SETUP 交換

- [ ] Publisher ↔ Relay 間で SETUP を交換し、セッションが確立する
- [ ] Subscriber ↔ Relay 間で SETUP を交換し、セッションが確立する
- [ ] 双方の control stream（unidirectional）が開かれる

---

## 4. Namespace 宣言と購読フロー

### 4.1 PUBLISH_NAMESPACE → REQUEST_OK

- [ ] Publisher が Relay に PUBLISH_NAMESPACE を送信し、Relay が REQUEST_OK を返す
- [ ] Relay が該当 namespace の Publisher としてセッションを記録する

### 4.2 SUBSCRIBE → SUBSCRIBE_OK

- [ ] Publisher が PUBLISH_NAMESPACE 済みの状態で、Subscriber が Relay に SUBSCRIBE を送信する
- [ ] Relay が namespace の一致する Publisher に SUBSCRIBE を転送する
- [ ] Publisher が SUBSCRIBE_OK を返し、Relay 経由で Subscriber に届く
- [ ] Relay が downstream の SUBSCRIBE_OK で Track Alias を返し、Object 転送時にその Alias が使われる（最小実装では Publisher が割り当てた Alias をそのまま使い回してよい）

### 4.3 SUBSCRIBE → REQUEST_ERROR

- [ ] PUBLISH_NAMESPACE されていない namespace の Track を SUBSCRIBE した場合、REQUEST_ERROR が返る

---

## 5. データ送信

### 5.1 Subgroup stream — 単一 Object

- [ ] Publisher が 1 Group / 1 Object を送信し、Subscriber が受信できる
- [ ] SUBGROUP_HEADER の Track Alias で正しいトラックが識別される（Relay は変換テーブルを通して転送する）
- [ ] Object の payload が Publisher の送信内容と一致する

### 5.2 Subgroup stream — 複数 Object

- [ ] Publisher が 1 Group 内に複数 Object を送信し、Subscriber が全て順序通り受信できる
- [ ] Object ID Delta が正しくエンコード/デコードされる

### 5.3 複数 Group

- [ ] Publisher が複数 Group を順次送信し、Subscriber が全て受信できる
- [ ] Group ごとに新しい unidirectional stream が開かれる（前提: 1 Group = 1 Subgroup = 1 stream）
- [ ] 各 Group の stream は全 Object 送信後に FIN で閉じられる（前の Group の FIN を待つ必要はない。複数 Group の stream が同時に開いていてよい）

### 5.4 複数 Track

- [ ] Publisher が映像 Track と音声 Track を同時に配信できる
- [ ] Subscriber が両方の Track を SUBSCRIBE し、両方の Object を受信できる
- [ ] Track Alias によって Track が正しく識別される

---

## 6. Relay の動作

### 6.1 複数 Subscriber

- [ ] 1 Publisher + 2 Subscriber の構成で、両方の Subscriber が同じ Object を受信できる
- [ ] Subscriber ごとに独立した Track Alias が割り当てられる

### 6.2 PUBLISH_DONE の転送

- [ ] Publisher が PUBLISH_DONE を送信し、Relay 経由で全 Subscriber に届く
- [ ] Subscriber が PUBLISH_DONE を受信した後、購読が終了する

### 6.3 切断ハンドリング

- [ ] Publisher が切断した場合、Relay が Subscriber に PUBLISH_DONE（または接続切断）を通知する
- [ ] Subscriber が切断した場合、Relay が該当 Subscriber の購読状態を破棄する（Publisher 側の配信は継続する）

---

## 7. エンドツーエンド

### 7.1 基本シナリオ

- [ ] Publisher がバイト列（疑似映像データ）を連続送信し、Subscriber が全て正しく受信できる
- [ ] 送信した payload と受信した payload がバイト単位で一致する

### 7.2 遅延参加

- [ ] Publisher が配信中に Subscriber が接続した場合、次の Group の先頭から Object を受信できる（NextGroupStart フィルタ）

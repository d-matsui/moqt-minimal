# 実装計画

## フェーズ 1: moqt-core — wire モジュール

- [x] workspace セットアップ（Cargo.toml）
- [x] varint エンコード/デコード + テスト
- [x] Track Namespace エンコード/デコード + テスト
- [x] Reason Phrase エンコード/デコード + テスト
- [x] Key-Value-Pair (Setup Options) エンコード/デコード + テスト

## フェーズ 2: moqt-core — message モジュール

- [x] SETUP + テスト
- [x] PUBLISH_NAMESPACE + テスト
- [x] REQUEST_OK + テスト
- [x] SUBSCRIBE + テスト（Message Parameter 含む）
- [x] SUBSCRIBE_OK + テスト（Message Parameter 含む）
- [x] REQUEST_ERROR + テスト
- [x] PUBLISH_DONE + テスト

## フェーズ 3: moqt-core — data モジュール

- [x] SUBGROUP_HEADER + テスト
- [x] Object fields + テスト（Object ID Delta resolution 含む）

## フェーズ 4: moqt-core — session モジュール

- [ ] ControlStreamWriter / ControlStreamReader
- [ ] RequestIdAllocator + テスト

## フェーズ 5: moqt-relay

- [ ] QUIC server 起動 + SETUP 交換
- [ ] PUBLISH_NAMESPACE 受信 → namespace 登録
- [ ] SUBSCRIBE 中継（downstream → upstream）
- [ ] Object stream 転送（upstream → downstream）
- [ ] PUBLISH_DONE 転送
- [ ] 結合テスト（session, subscription）

## フェーズ 6: moqt-pub / moqt-sub

- [ ] moqt-pub: 接続 → PUBLISH_NAMESPACE → SUBSCRIBE 応答 → Object 送信
- [ ] moqt-sub: 接続 → SUBSCRIBE → Object 受信
- [ ] 結合テスト（data_transfer, relay, e2e）

## 現在のフェーズ

**フェーズ 1** — 未着手

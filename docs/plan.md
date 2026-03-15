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

- [x] ControlStreamWriter / ControlStreamReader
- [x] RequestIdAllocator + テスト

## フェーズ 5: moqt-relay

- [x] QUIC server 起動 + SETUP 交換
- [x] PUBLISH_NAMESPACE 受信 → namespace 登録
- [x] SUBSCRIBE 中継（downstream → upstream）
- [x] Object stream 転送（upstream → downstream）
- [x] PUBLISH_DONE 転送
- [x] 結合テスト（session, subscription, object forwarding, publish_done）

## フェーズ 6: moqt-pub / moqt-sub

- [x] moqt-pub: 接続 → PUBLISH_NAMESPACE → SUBSCRIBE 応答 → Object 送信
- [x] moqt-sub: 接続 → SUBSCRIBE → Object 受信
- [x] 結合テスト（multiple_groups, late_join）

## 現在のフェーズ

**全フェーズ完了** — 82テスト全パス

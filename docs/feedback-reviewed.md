# draft-17 vs ドキュメント整合性チェック レビュー結果

2026-03-15 に実施した整合性チェック結果に対する批判的レビュー。

## 要修正（実装に直接影響）

### 1. Message Parameter と Key-Value-Pair のエンコーディング混同 【message 実装フェーズで対処】
- **箇所**: `minimal-moqt-spec.md` §5.4, `design-moqt-core.md`
- **問題**: Key-Value-Pair（§1.4.3: 偶数→varint、奇数→length-prefixed）のみ定義され、Message Parameter（§9.3: パラメータ定義ごとに異なるエンコーディング）が未定義
- **仕様根拠**: draft-17 §9.3 (lines 2622-2717) vs §1.4.3 (lines 592-632)
- **対応**: この最小実装で使う Message Parameter は SUBSCRIPTION_FILTER と LARGEST_OBJECT 程度なので、汎用 dispatcher ではなく必要なパラメータのみ個別に実装する

### 2. SUBSCRIPTION_FILTER のワイヤフォーマット未記載 【message 実装フェーズで対処】
- **箇所**: `minimal-moqt-spec.md` §3.2
- **問題**: パラメータ値 (0x21) と Filter Type (NextGroupStart=0x1) は正しく記載されているが、Message Parameter としてのエンコード方法（length-prefixed: Parameter Type + Length + Filter Type）が未記載
- **仕様根拠**: draft-17 §9.3.7
- **補足**: NextGroupStart は optional fields なしのため実害は小さいが、フォーマット仕様として明記すべき

### 3. Object ID Delta のセマンティクスが仕様と異なる 【data stream 実装フェーズで対処】
- **箇所**: `minimal-moqt-spec.md` §4.1
- **問題**: 「前の Object ID との差分」と記載。draft-17 では `Object ID = 前の Object ID + Delta + 1`。連続 ID (0,1,2,...) なら全 delta=0
- **仕様根拠**: draft-17 §10.4.2 (lines 4829-4836)

### 4. Relay の状態管理で session に role フィールドがある矛盾 【relay 実装フェーズで対処】
- **箇所**: `minimal-moqt-spec.md` §6.5
- **問題**: Sessions に `role (publisher|subscriber)` があるが、§6.1 と `design-moqt-relay.md` では「接続時点では role を決定しない（トラック単位）」と明記
- **対応**: Sessions から role フィールドを削除し、トラック単位の設計に統一する

## 要修正（中程度）

### 5. LARGEST_OBJECT のエンコーディング不明確 【message 実装フェーズで対処】
- **箇所**: `minimal-moqt-spec.md` §3.3
- **問題**: Location 型（Group vi64 + Object vi64）であることが未記載
- **仕様根拠**: draft-17 §9.3.9 (lines 3042-3047), §1.4.2 (lines 576-579)

### 6. Retry Interval の "+1" セマンティクス欠落 【message 実装フェーズで対処】
- **箇所**: `minimal-moqt-spec.md` §3.6
- **問題**: 0=リトライ不可は正しいが、値が「ミリ秒+1」であることが未記載。1=即時リトライ
- **仕様根拠**: draft-17 §9.7 (lines 3369-3371, 3385-3386)

## 軽微・補足推奨

### 7. DOES_NOT_EXIST エラーコード値の未記載
- 仕様上 **0x10** と定義（draft-17 line 5949）。ドキュメントに明記すべき

### 8. design-moqt-core のモジュール構成に key_value_pair.rs が欠落
- design-overview のテスト構成では参照あり。設計ドキュメントの整合性の問題

### 9. SUBGROUP_HEADER Type 値 0x30 → 0x38 の訂正過程 【要再確認】
- 元の指摘は「訂正過程が残っている」だが、具体的箇所が不明確でアクション不能。最終結論 0x38 は正しい
- 実際にドキュメント中に混乱を招く記述があるか再確認が必要

### 10. vi64 の 7バイトエンコーディング不在 【記述修正】
- 元の記述「11111100 は無効コードポイント」は不正確。正確には vi64 は 1/2/3/4/5/6/8/9 バイトの8形式で、7バイト形式がスキップされている（prefix が `111111` → `11111110` に飛ぶ）
- ドキュメントへの追記は nice-to-have。実装上はデコーダのテストで担保すべき

## 整合確認済み（問題なし）

- メッセージタイプ値: 全て正しい (SETUP=0x2F00, SUBSCRIBE=0x3, SUBSCRIBE_OK=0x4, REQUEST_ERROR=0x5, PUBLISH_NAMESPACE=0x6, REQUEST_OK=0x7, PUBLISH_DONE=0xB)
- Control Message フレーミング: Type(vi64) + Length(16bit固定) + Payload
- セッション初期化: 各 peer が uni stream を開き SETUP を送信
- Request ID 採番: client 偶数, server 奇数
- Request stream: 1リクエスト = 1 bidi stream
- FORWARD(0x10) デフォルト値 1
- SUBGROUP_HEADER Type = 0x38
- 全体のメッセージフロー

## 未確認の観点

- 既存実装コード (`moqt-core/src/`) が上記の問題を既に含んでいるか未検証（特に `key_value_pair.rs` が Message Parameter の代わりに使われていないか）
- `plan.md` の実装計画との紐づけが未整理

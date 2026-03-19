# 設計判断

## wire/message/data 層でのフィールド取り扱い

decode 時にフィールドをどう扱うかは、以下の3パターンで判断する。

| パターン | 条件 | 実装 | 例 |
|---------|------|------|-----|
| struct に持つ | 値を解釈して使う、または差し替えて再 encode する | フィールドとして decode/encode | SubgroupHeader の track_alias, subgroup_id, publisher_priority |
| 生バイト列で保持 | 転送するが中身は解釈しない | raw bytes で保持 | subscribe_ok の track_properties_raw |
| 読み飛ばす | 転送しない、または Relay が生バイト列で丸ごと転送するため decode 側での保持が不要 | バッファを進めるだけ | Message Parameters の未使用パラメータ、Object Properties |

### 判断の根拠（仕様）

- **Message Parameters**: "intended for the peer only and are not forwarded by Relays" (Section 9.3.1) → 読み飛ばし
- **Track Properties**: Relay が Subscriber に転送する (Section 2.5) → 生バイト列で保持
- **Object Properties / Object データ**: Relay は decode 前の生バイト列をそのまま転送する設計のため、decode 側では読み飛ばしで十分

### Relay の転送設計

Relay は SubgroupHeader と Object を「decode 前の生バイト列を保持してそのまま転送」する。decode は Track Alias の特定など最低限の情報取得のためだけに行う。そのため、転送が必要なフィールドでも decode 側で保持する必要がない場合がある（Object Properties がこれに該当）。

ただし SubgroupHeader は Track Alias を差し替えて再 encode する必要があるため、全フィールドを struct に持つ。

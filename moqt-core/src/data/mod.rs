//! # data: MOQT データストリームの型定義
//!
//! メディアデータは QUIC 単方向ストリーム（uni stream）で送信される。
//! 各ストリームは以下の構造を持つ:
//!
//! ```text
//! [SubgroupHeader]  ← ストリームの先頭に1回
//! [ObjectHeader + Payload]  ← オブジェクトが0個以上続く
//! [ObjectHeader + Payload]
//! ...
//! [FIN]  ← ストリーム終了
//! ```
//!
//! ## Group と Object の関係
//! - Group: 独立してデコード可能なメディアデータの単位（例: キーフレームから始まる映像セグメント）
//! - Object: Group 内の個々のデータ単位（例: 1つの映像フレーム）
//! - Subgroup: Group 内のサブ分割（最小実装では1つの Group に1つの Subgroup）

pub mod object;
pub mod subgroup_header;

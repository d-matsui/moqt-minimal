//! # wire: MOQT ワイヤーフォーマットの基本型
//!
//! MOQT プロトコルでバイト列としてやり取りされる基本的なデータ型を定義する。
//! これらは上位のメッセージ型から部品として使われる。
//!
//! - `varint`: 可変長整数のエンコード・デコード（プロトコル全体で多用される）
//! - `track_namespace`: トラック名前空間（パブリッシャーが配信するメディアの識別子）
//! - `reason_phrase`: エラー理由等の長さ付き UTF-8 文字列
//! - `key_value_pair`: SETUP メッセージ等で使われるキー・バリューペア

pub mod key_value_pair;
pub mod reason_phrase;
pub mod track_namespace;
pub mod varint;

//! # moqt-core: MOQT (Media over QUIC Transport) プロトコルのコアライブラリ
//!
//! このクレートは MOQT draft-17 仕様に基づくプロトコルメッセージの
//! エンコード・デコード機能を提供する。
//!
//! ## モジュール構成
//! - `primitives`: 可変長整数 (varint) やトラック名前空間など、プロトコルの基本型
//! - `message`: SETUP, SUBSCRIBE, PUBLISH_NAMESPACE などの制御メッセージ
//! - `data`: サブグループヘッダーやオブジェクトなど、メディアデータストリーム用の型
//! - `stream`: QUIC ストリーム上のフレーミング（読み書き）
//! - `session`: プロトコルロジックと高レベル API（SETUP ハンドシェイク、イベントディスパッチ）

pub mod client;
pub mod data;
pub mod message;
pub mod primitives;
pub mod quic_config;
pub mod session;
pub mod stream;

//! # moqt-core: MOQT (Media over QUIC Transport) プロトコルのコアライブラリ
//!
//! このクレートは MOQT draft-17 仕様に基づくプロトコルメッセージの
//! エンコード・デコード機能を提供する。
//!
//! ## モジュール構成
//! - `primitives`: 可変長整数 (varint) やトラック名前空間など、プロトコルの基本型
//! - `message`: SETUP, SUBSCRIBE, PUBLISH_NAMESPACE などの制御メッセージ
//! - `data`: サブグループヘッダーやオブジェクトなど、メディアデータストリーム用の型
//! - `session`: QUIC セッション管理（コントロールストリーム、リクエストID 割り当て、QUIC設定）

pub mod data;
pub mod message;
pub mod primitives;
pub mod quic_config;
pub mod session;

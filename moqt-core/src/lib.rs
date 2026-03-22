//! # moqt-core: MOQT (Media over QUIC Transport) プロトコルのコアライブラリ
//!
//! このクレートは MOQT draft-17 仕様に基づくプロトコルメッセージの
//! エンコード・デコード機能を提供する。
//!
//! ## モジュール構成
//! - `wire`: ワイヤフォーマット定義（プリミティブ型、制御メッセージ、データストリームヘッダ）
//! - `stream`: ストリーム上のフレーミング（読み書き）
//! - `session`: プロトコルロジックと高レベル API（SETUP ハンドシェイク、イベントディスパッチ）

pub mod client;
pub mod quic_config;
pub mod session;
pub mod stream;
pub mod wire;

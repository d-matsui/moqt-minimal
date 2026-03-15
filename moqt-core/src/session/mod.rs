//! # session: MOQT セッション管理
//!
//! QUIC 接続上での MOQT セッションを管理するためのユーティリティ。
//!
//! - `control_stream`: コントロールストリームの読み書き（非同期 QUIC ストリーム対応）
//! - `request_id`: リクエスト ID の割り当て（クライアント=偶数、サーバー=奇数）
//! - `quic_config`: QUIC/TLS の設定ヘルパー

pub mod control_stream;
pub mod quic_config;
pub mod request_id;

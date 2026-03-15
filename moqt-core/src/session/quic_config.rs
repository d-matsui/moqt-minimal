//! # quic_config: QUIC/TLS 設定ヘルパー
//!
//! MOQT は QUIC 上で動作するため、QUIC 接続の TLS 設定が必要。
//! このモジュールは、サーバー側とクライアント側の設定を簡単に作成する
//! ヘルパー関数を提供する。
//!
//! ## ALPN (Application-Layer Protocol Negotiation)
//! TLS ハンドシェイク時に、両端が MOQT プロトコルを使うことを合意するために
//! ALPN プロトコル ID として "moqt-17" を指定する。

use std::sync::Arc;

/// MOQT draft-17 用の ALPN プロトコル識別子。
/// TLS ハンドシェイクで互いにプロトコルを確認するために使用される。
pub const ALPN: &[u8] = b"moqt-17";

/// 指定された証明書と秘密鍵を使って QUIC サーバー設定を作成する。
/// リレーサーバーで使用される。
pub fn make_server_config(
    cert_der: rustls_pki_types::CertificateDer<'static>,
    key_der: rustls_pki_types::PrivateKeyDer<'static>,
) -> quinn::ServerConfig {
    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("failed to build server TLS config");
    server_crypto.alpn_protocols = vec![ALPN.to_vec()];

    let quic_server_config =
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto).unwrap();
    quinn::ServerConfig::with_crypto(Arc::new(quic_server_config))
}

/// 指定された証明書を信頼する QUIC クライアント設定を作成する。
/// パブリッシャーやサブスクライバーで使用される。
///
/// 本番環境では CA 署名の証明書を使うが、開発時は自己署名証明書を
/// この関数で信頼させる。
pub fn make_client_config(
    cert_der: rustls_pki_types::CertificateDer<'static>,
) -> quinn::ClientConfig {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add(cert_der).expect("failed to add cert");

    let mut client_crypto = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![ALPN.to_vec()];

    let quic_client_config =
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    quinn::ClientConfig::new(Arc::new(quic_client_config))
}

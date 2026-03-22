//! # transport: Transport abstraction for MOQT
//!
//! Defines traits that abstract over the underlying transport (raw QUIC, WebTransport, etc.).
//! The `stream/` and `session/` modules use these traits via `Box<dyn ...>` so that
//! the MOQT protocol logic is transport-agnostic.
//!
//! - `SendStream`: writable byte stream (write_all, finish)
//! - `RecvStream`: readable byte stream (read_exact)
//! - `Connection`: open/accept streams

pub mod quic;

use std::future::Future;
use std::io;
use std::pin::Pin;

/// A boxed future that is Send.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A boxed stream pair (send, recv).
pub type StreamPair = (Box<dyn SendStream>, Box<dyn RecvStream>);

/// A writable byte stream.
///
/// Abstracts over `quinn::SendStream`, `wtransport::SendStream`, etc.
pub trait SendStream: Send + Sync + Unpin {
    /// Write all bytes to the stream.
    fn write_all<'a>(
        &'a mut self,
        buf: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;

    /// Signal stream completion (send FIN).
    fn finish(&mut self) -> io::Result<()>;
}

/// A readable byte stream.
///
/// Abstracts over `quinn::RecvStream`, `wtransport::RecvStream`, etc.
/// `read_exact` returns `io::ErrorKind::UnexpectedEof` when the stream ends
/// before enough bytes are read. This is the canonical way to detect stream FIN.
pub trait RecvStream: Send + Sync + Unpin {
    /// Read exactly `buf.len()` bytes from the stream.
    /// Returns `io::ErrorKind::UnexpectedEof` if the stream ends before enough bytes.
    fn read_exact<'a>(
        &'a mut self,
        buf: &'a mut [u8],
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;
}

/// A transport connection that can open and accept QUIC-like streams.
///
/// Abstracts over `quinn::Connection`, WebTransport sessions, etc.
pub trait Connection: Send + Sync {
    /// Open a unidirectional stream for sending.
    fn open_uni(&self) -> BoxFuture<'_, io::Result<Box<dyn SendStream>>>;

    /// Accept an incoming unidirectional stream.
    fn accept_uni(&self) -> BoxFuture<'_, io::Result<Box<dyn RecvStream>>>;

    /// Open a bidirectional stream.
    fn open_bi(&self) -> BoxFuture<'_, io::Result<StreamPair>>;

    /// Accept an incoming bidirectional stream.
    fn accept_bi(&self) -> BoxFuture<'_, io::Result<StreamPair>>;

    /// Close the connection.
    fn close(&self, code: u32, reason: &[u8]);
}

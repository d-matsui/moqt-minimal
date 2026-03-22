//! # quic: Transport trait implementations for quinn (raw QUIC)

use std::io;

use crate::transport::{self, BoxFuture, StreamPair};

// === SendStream ===

impl transport::SendStream for quinn::SendStream {
    fn write_all<'a>(&'a mut self, buf: &'a [u8]) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            quinn::SendStream::write_all(self, buf)
                .await
                .map_err(io::Error::other)
        })
    }

    fn finish(&mut self) -> io::Result<()> {
        quinn::SendStream::finish(self).map_err(io::Error::other)
    }
}

// === RecvStream ===

impl transport::RecvStream for quinn::RecvStream {
    fn read_exact<'a>(&'a mut self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            quinn::RecvStream::read_exact(self, buf)
                .await
                .map_err(|e| match e {
                    quinn::ReadExactError::FinishedEarly(_) => {
                        io::Error::new(io::ErrorKind::UnexpectedEof, e)
                    }
                    quinn::ReadExactError::ReadError(re) => io::Error::other(re),
                })
        })
    }
}

// === Connection ===

impl transport::Connection for quinn::Connection {
    fn open_uni(&self) -> BoxFuture<'_, io::Result<Box<dyn transport::SendStream>>> {
        Box::pin(async move {
            let stream = quinn::Connection::open_uni(self)
                .await
                .map_err(io::Error::other)?;
            Ok(Box::new(stream) as Box<dyn transport::SendStream>)
        })
    }

    fn accept_uni(&self) -> BoxFuture<'_, io::Result<Box<dyn transport::RecvStream>>> {
        Box::pin(async move {
            let stream = quinn::Connection::accept_uni(self)
                .await
                .map_err(io::Error::other)?;
            Ok(Box::new(stream) as Box<dyn transport::RecvStream>)
        })
    }

    fn open_bi(&self) -> BoxFuture<'_, io::Result<StreamPair>> {
        Box::pin(async move {
            let (send, recv) = quinn::Connection::open_bi(self)
                .await
                .map_err(io::Error::other)?;
            Ok((
                Box::new(send) as Box<dyn transport::SendStream>,
                Box::new(recv) as Box<dyn transport::RecvStream>,
            ))
        })
    }

    fn accept_bi(&self) -> BoxFuture<'_, io::Result<StreamPair>> {
        Box::pin(async move {
            let (send, recv) = quinn::Connection::accept_bi(self)
                .await
                .map_err(io::Error::other)?;
            Ok((
                Box::new(send) as Box<dyn transport::SendStream>,
                Box::new(recv) as Box<dyn transport::RecvStream>,
            ))
        })
    }

    fn close(&self, code: u32, reason: &[u8]) {
        quinn::Connection::close(self, code.into(), reason);
    }
}

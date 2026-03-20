//! # request_id: MOQT request ID allocation
//!
//! MOQT assigns a unique ID to each request (SUBSCRIBE, PUBLISH_NAMESPACE, etc.).
//! Even/odd parity distinguishes the originator:
//! - Client (publisher/subscriber): even (0, 2, 4, ...)
//! - Server (relay): odd (1, 3, 5, ...)
//!
//! This scheme ensures both sides can independently generate IDs without collision.

use std::sync::atomic::{AtomicU64, Ordering};

/// Request ID allocator.
/// Produces even IDs for clients, odd IDs for servers.
/// Uses atomic operations so it can be shared across tasks via `&self`.
pub struct RequestIdAllocator {
    next_id: AtomicU64,
}

impl RequestIdAllocator {
    /// Create a client allocator (even IDs: 0, 2, 4, ...).
    pub fn client() -> Self {
        Self {
            next_id: AtomicU64::new(0),
        }
    }

    /// Create a server allocator (odd IDs: 1, 3, 5, ...).
    pub fn server() -> Self {
        Self {
            next_id: AtomicU64::new(1),
        }
    }

    /// Allocate the next request ID. Increments by 2 each time.
    pub fn allocate(&self) -> u64 {
        self.next_id.fetch_add(2, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_generates_even() {
        let alloc = RequestIdAllocator::client();
        assert_eq!(alloc.allocate(), 0);
        assert_eq!(alloc.allocate(), 2);
        assert_eq!(alloc.allocate(), 4);
    }

    #[test]
    fn server_generates_odd() {
        let alloc = RequestIdAllocator::server();
        assert_eq!(alloc.allocate(), 1);
        assert_eq!(alloc.allocate(), 3);
        assert_eq!(alloc.allocate(), 5);
    }
}

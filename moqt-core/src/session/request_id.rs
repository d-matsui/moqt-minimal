//! # request_id: MOQT request ID allocation
//!
//! MOQT assigns a unique ID to each request (SUBSCRIBE, PUBLISH_NAMESPACE, etc.).
//! Even/odd parity distinguishes the originator:
//! - Client (publisher/subscriber): even (0, 2, 4, ...)
//! - Server (relay): odd (1, 3, 5, ...)
//!
//! This scheme ensures both sides can independently generate IDs without collision.

/// Request ID allocator.
/// Produces even IDs for clients, odd IDs for servers.
pub struct RequestIdAllocator {
    next_id: u64,
}

impl RequestIdAllocator {
    /// Create a client allocator (even IDs: 0, 2, 4, ...).
    pub fn client() -> Self {
        Self { next_id: 0 }
    }

    /// Create a server allocator (odd IDs: 1, 3, 5, ...).
    pub fn server() -> Self {
        Self { next_id: 1 }
    }

    /// Allocate the next request ID. Increments by 2 each time.
    pub fn allocate(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 2;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_generates_even() {
        let mut alloc = RequestIdAllocator::client();
        assert_eq!(alloc.allocate(), 0);
        assert_eq!(alloc.allocate(), 2);
        assert_eq!(alloc.allocate(), 4);
    }

    #[test]
    fn server_generates_odd() {
        let mut alloc = RequestIdAllocator::server();
        assert_eq!(alloc.allocate(), 1);
        assert_eq!(alloc.allocate(), 3);
        assert_eq!(alloc.allocate(), 5);
    }
}

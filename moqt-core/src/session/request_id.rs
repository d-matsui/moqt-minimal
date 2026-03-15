/// Allocates Request IDs per the MOQT spec (Section 9.1).
/// Client generates even IDs (0, 2, 4, ...), server generates odd IDs (1, 3, 5, ...).
pub struct RequestIdAllocator {
    next_id: u64,
}

impl RequestIdAllocator {
    /// Create an allocator for a client (even IDs starting at 0).
    pub fn client() -> Self {
        Self { next_id: 0 }
    }

    /// Create an allocator for a server (odd IDs starting at 1).
    pub fn server() -> Self {
        Self { next_id: 1 }
    }

    /// Allocate the next Request ID.
    pub fn next(&mut self) -> u64 {
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
        assert_eq!(alloc.next(), 0);
        assert_eq!(alloc.next(), 2);
        assert_eq!(alloc.next(), 4);
    }

    #[test]
    fn server_generates_odd() {
        let mut alloc = RequestIdAllocator::server();
        assert_eq!(alloc.next(), 1);
        assert_eq!(alloc.next(), 3);
        assert_eq!(alloc.next(), 5);
    }
}

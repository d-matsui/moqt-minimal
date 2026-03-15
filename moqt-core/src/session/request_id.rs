//! # request_id: MOQT リクエスト ID の割り当て
//!
//! MOQT では、SUBSCRIBE や PUBLISH_NAMESPACE などのリクエストに
//! 一意な ID を割り当てる。ID の偶奇でリクエストの発信元を区別する:
//! - クライアント（パブリッシャー/サブスクライバー）: 偶数（0, 2, 4, ...）
//! - サーバー（リレー）: 奇数（1, 3, 5, ...）
//!
//! この方式により、双方が独立して ID を生成しても衝突しない。

/// リクエスト ID のアロケータ。
/// クライアント用（偶数）とサーバー用（奇数）を使い分ける。
pub struct RequestIdAllocator {
    next_id: u64,
}

impl RequestIdAllocator {
    /// クライアント用アロケータ（偶数 ID: 0, 2, 4, ...）を作成。
    pub fn client() -> Self {
        Self { next_id: 0 }
    }

    /// サーバー用アロケータ（奇数 ID: 1, 3, 5, ...）を作成。
    pub fn server() -> Self {
        Self { next_id: 1 }
    }

    /// 次のリクエスト ID を割り当てる。毎回 2 ずつ増加する。
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

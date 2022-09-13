use std::sync::atomic::{self, AtomicU64};

use serde::Serialize;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ClientId(pub u64);

impl ClientId {
    pub fn new() -> Self {
        static CUR_CLIENT_ID: AtomicU64 = AtomicU64::new(0);
        Self(CUR_CLIENT_ID.fetch_add(1, atomic::Ordering::Relaxed))
    }
}

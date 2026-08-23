//! Moved to the `preceipts-proto` crate, where the app can speak it without
//! taking on this crate's dependencies. Re-exported under the old path so
//! every `crate::proto::…` in the daemon still resolves.

pub use preceipts_proto::*;

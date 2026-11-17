#[cfg(any(target_os = "android", test))]
mod android_store;
pub mod bundle;
pub(crate) mod downloads;
pub mod error;
pub mod files;
mod identity;
mod protocol;
pub(crate) mod resume;
mod store;
mod transport;

mod service;
pub use service::{EVENT, Service, Snapshot};

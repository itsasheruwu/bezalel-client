#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
mod install;
mod types;

#[cfg(feature = "client")]
pub use client::SyncClient;
#[cfg(feature = "client")]
pub use install::{InstalledRelease, PackLock};
pub use types::*;

pub type SyncError = String;

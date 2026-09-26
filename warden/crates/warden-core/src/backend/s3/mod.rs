//! S3-compatible backend: hand-rolled minimal client (SigV4 + ureq),
//! path-style addressing, same key layout as the local backend. The
//! in-process mock (`mock` module) verifies real signatures and doubles as
//! the conformance fixture; real-S3/MinIO integration is a user-side step
//! documented in the README.

pub mod client;
pub mod mock;
pub mod sigv4;

pub use client::{S3Backend, S3Config};

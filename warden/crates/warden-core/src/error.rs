use thiserror::Error;

/// Library-wide error type. `Corrupt` is deliberately loud and names the
/// object: detection with attribution is the product's core promise.
#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("crypto: {0}")]
    Crypto(&'static str),

    #[error("corrupt object {name}: {reason}")]
    Corrupt { name: String, reason: &'static str },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("repository is locked by writer pid {pid} until {expires_unix} (unix); retry later or wait for expiry")]
    Locked { pid: u32, expires_unix: u64 },

    #[error("format: {0}")]
    Format(String),

    #[error("passphrase incorrect or keyfile damaged")]
    BadPassphrase,

    #[error("destination already exists: {0}")]
    DestExists(String),
}

pub type Result<T> = std::result::Result<T, Error>;

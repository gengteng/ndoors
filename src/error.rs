#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid operation")]
    InvalidOperation,
    #[error("Invalid door index")]
    InvalidDoorIndex,
}

pub type Result<T> = std::result::Result<T, Error>;

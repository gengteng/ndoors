#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid operation")]
    InvalidOperation,
    #[error("Invalid door index")]
    InvalidDoorIndex,
    #[error("Impossible")]
    Impossible,
}

pub type Result<T> = std::result::Result<T, Error>;

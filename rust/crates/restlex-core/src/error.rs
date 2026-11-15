use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("restic: {0}")]
    Restic(String),
    /// Невалидный ввод клиента (PATCH настроек) — это 400, а не 500.
    #[error("bad patch: {0}")]
    BadPatch(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

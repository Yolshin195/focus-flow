//! Ошибки хранилища. В порты ядра уходят как [`CoreError::Storage`].

use focus_core::CoreError;

/// Результат операции хранилища.
pub type StorageResult<T> = Result<T, StorageError>;

/// Ошибка хранилища.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Ошибка SQLite / sqlx.
    #[error("SQLite: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Ошибка миграции схемы.
    #[error("миграция: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// Значение в БД не читается как доменное.
    #[error("неверные данные в БД: {0}")]
    Decode(String),
    /// Ошибка JSON (настройки, события outbox).
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Ошибка файловой системы (каталог БД).
    #[error("файл БД: {0}")]
    Io(#[from] std::io::Error),
}

impl From<StorageError> for CoreError {
    fn from(e: StorageError) -> Self {
        tracing::error!(error = %e, "ошибка хранилища");
        CoreError::storage(e)
    }
}

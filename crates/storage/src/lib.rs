//! Хранилище Focus Flow: SQLite через `sqlx`, миграции ядра, реализации
//! репозиториев ядра, дерево задач и таблица `outbox`.
//!
//! ```no_run
//! # async fn f() -> Result<(), focus_storage::StorageError> {
//! use std::sync::Arc;
//! use focus_core::{FocusService, NoTaskSources, SystemClock};
//!
//! let storage = focus_storage::Storage::open("focus-flow.db").await?;
//! let svc = FocusService::new(storage.ports(Arc::new(SystemClock), Arc::new(NoTaskSources)));
//! # Ok(()) }
//! ```
//!
//! Запросы — runtime (`sqlx::query`), без проверки на этапе компиляции и
//! без `DATABASE_URL`. Схема — `migrations/`, применяется при открытии.

mod codec;
mod error;
mod repos;

use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use focus_core::{Clock, Ports, TaskSources};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub use error::{StorageError, StorageResult};

/// Миграции ядра (таблицы `work_days`, `tasks`, `sessions`, `settings`, `outbox`).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Сколько ждать снятия блокировки БД другим соединением.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Соединений в пуле файла (WAL: читатели не мешают писателю).
const FILE_POOL_SIZE: u32 = 4;

/// Хранилище: пул соединений SQLite и реализации всех портов ядра.
/// Дёшево клонируется.
#[derive(Debug, Clone)]
pub struct Storage {
    pool: SqlitePool,
    /// Подписчики outbox (id включённых плагинов) — задаёт сервер при старте.
    subscribers: Arc<RwLock<Vec<String>>>,
}

impl Storage {
    /// Открыть (или создать) файл БД и применить миграции. Каталог файла
    /// создаётся, если его нет. WAL, `busy_timeout` 5 с, внешние ключи включены.
    pub async fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let path = path.as_ref();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let opts = base_options()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);
        let pool = SqlitePoolOptions::new()
            .max_connections(FILE_POOL_SIZE)
            .connect_with(opts)
            .await?;
        tracing::info!(path = %path.display(), "открыта БД");
        Self::init(pool).await
    }

    /// БД в памяти (для тестов): одно соединение, которое живёт, пока жив
    /// пул, — иначе данные пропадут вместе с соединением.
    pub async fn open_in_memory() -> StorageResult<Self> {
        let opts = base_options().in_memory(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await?;
        Self::init(pool).await
    }

    async fn init(pool: SqlitePool) -> StorageResult<Self> {
        MIGRATOR.run(&pool).await?;
        Ok(Self {
            pool,
            subscribers: Arc::default(),
        })
    }

    /// Пул соединений (для миграций и таблиц плагинов).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Закрыть все соединения (дождавшись их возврата в пул).
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Порты ядра поверх этого хранилища.
    pub fn ports(&self, clock: Arc<dyn Clock>, sources: Arc<dyn TaskSources>) -> Ports {
        let me = Arc::new(self.clone());
        Ports {
            clock,
            days: me.clone(),
            tasks: me.clone(),
            sessions: me.clone(),
            settings: me.clone(),
            events: me,
            sources,
        }
    }
}

fn base_options() -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT)
}

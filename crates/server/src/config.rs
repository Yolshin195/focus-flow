//! Конфигурация сервиса: адрес, data dir, разрешённые `Host` и `Origin`.
//!
//! Порядок: значения по умолчанию → переменные окружения
//! (`FOCUS_ADDR`, `FOCUS_DATA_DIR`, `FOCUS_ALLOWED_HOSTS`, `FOCUS_ALLOWED_ORIGINS`)
//! → флаги CLI (их накладывает `bin/focus-flow`).

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;

use anyhow::Context as _;

/// Адрес по умолчанию: только loopback.
pub const DEFAULT_ADDR: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7878));

/// Имя файла БД в data dir.
const DB_FILE: &str = "focus.db";
/// Имя файла токена в data dir.
const TOKEN_FILE: &str = "token";
/// Каталог секретов плагинов в data dir.
const SECRETS_DIR: &str = "secrets";

/// Конфигурация сервиса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Адрес, на котором слушает сервис.
    pub addr: SocketAddr,
    /// Каталог данных: `focus.db`, `token`.
    pub data_dir: PathBuf,
    /// Дополнительные разрешённые значения `Host` (без порта), кроме
    /// `127.0.0.1`, `localhost`, `[::1]` — например, имя VDS.
    pub allowed_hosts: Vec<String>,
    /// Дополнительные разрешённые `Origin` (точное значение или `scheme://`
    /// для любой страницы схемы), кроме встроенных для desktop webview.
    pub allowed_origins: Vec<String>,
}

impl Config {
    /// Значения по умолчанию: `127.0.0.1:7878`, системный data dir.
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self::with_data_dir(default_data_dir()?))
    }

    /// Значения по умолчанию с заданным data dir.
    pub fn with_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            addr: DEFAULT_ADDR,
            data_dir: data_dir.into(),
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
        }
    }

    /// Значения по умолчанию + переменные окружения процесса.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// То же, что [`Config::from_env`], с произвольным источником переменных.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let data_dir = match var("FOCUS_DATA_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => default_data_dir()?,
        };
        let mut cfg = Self::with_data_dir(data_dir);
        if let Some(addr) = var("FOCUS_ADDR") {
            cfg.addr = addr.parse().with_context(|| {
                format!("FOCUS_ADDR = «{addr}»: ожидается адрес вида 127.0.0.1:7878")
            })?;
        }
        if let Some(hosts) = var("FOCUS_ALLOWED_HOSTS") {
            cfg.allowed_hosts = split_list(&hosts);
        }
        if let Some(origins) = var("FOCUS_ALLOWED_ORIGINS") {
            cfg.allowed_origins = split_list(&origins);
        }
        Ok(cfg)
    }

    /// Путь к файлу БД.
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join(DB_FILE)
    }

    /// Каталог секретов плагинов (`secrets/<id>/`, права `0700`/`0600`).
    pub fn secrets_dir(&self) -> PathBuf {
        self.data_dir.join(SECRETS_DIR)
    }

    /// Путь к файлу токена.
    pub fn token_path(&self) -> PathBuf {
        self.data_dir.join(TOKEN_FILE)
    }
}

/// Системный каталог данных приложения (`directories`): Linux —
/// `~/.local/share/focus-flow`, macOS — `~/Library/Application Support/dev.FocusFlow.focus-flow`.
pub fn default_data_dir() -> anyhow::Result<PathBuf> {
    directories::ProjectDirs::from("dev", "FocusFlow", "focus-flow")
        .map(|d| d.data_dir().to_path_buf())
        .context("не удалось определить каталог данных (нет домашнего каталога?); задайте FOCUS_DATA_DIR")
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn env_overrides_defaults() {
        let cfg = Config::from_vars(vars(&[
            ("FOCUS_DATA_DIR", "/tmp/ff"),
            ("FOCUS_ADDR", "127.0.0.1:9000"),
            ("FOCUS_ALLOWED_HOSTS", "focus.example.com, ,vds"),
            ("FOCUS_ALLOWED_ORIGINS", "https://focus.example.com"),
        ]))
        .unwrap();
        assert_eq!(cfg.addr, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(cfg.db_path(), PathBuf::from("/tmp/ff/focus.db"));
        assert_eq!(cfg.token_path(), PathBuf::from("/tmp/ff/token"));
        assert_eq!(cfg.allowed_hosts, ["focus.example.com", "vds"]);
        assert_eq!(cfg.allowed_origins, ["https://focus.example.com"]);
    }

    #[test]
    fn defaults_without_env() {
        let cfg = Config::from_vars(vars(&[("FOCUS_DATA_DIR", "/d")])).unwrap();
        assert_eq!(cfg, Config::with_data_dir("/d"));
        assert_eq!(cfg.addr, DEFAULT_ADDR);
        assert_eq!(cfg.addr.to_string(), "127.0.0.1:7878");
    }

    #[test]
    fn bad_addr_is_error() {
        let err = Config::from_vars(vars(&[("FOCUS_DATA_DIR", "/d"), ("FOCUS_ADDR", "nope")]));
        assert!(err.is_err());
    }
}

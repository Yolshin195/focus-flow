//! CLI Focus Flow: `focus-flow serve` — локальный сервис, `focus-flow token` —
//! показать токен доступа, `focus-flow ui` — desktop-клиент (поднимет `serve`,
//! если тот не запущен).

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[cfg(feature = "ui")]
mod ui;
use focus_server::{Config, load_or_create_token};

/// Трекер фокуса 50/10.
#[derive(Debug, Parser)]
#[command(name = "focus-flow", version, about = "Трекер фокуса 50/10")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Общие флаги: перекрывают `FOCUS_ADDR` / `FOCUS_DATA_DIR`.
#[derive(Debug, Args)]
struct ConfigArgs {
    /// Адрес сервиса (по умолчанию 127.0.0.1:7878; переменная FOCUS_ADDR).
    #[arg(long)]
    addr: Option<SocketAddr>,
    /// Каталог данных: focus.db и token (переменная FOCUS_DATA_DIR).
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

impl ConfigArgs {
    fn config(self) -> anyhow::Result<Config> {
        // Флаг `--data-dir` подменяет FOCUS_DATA_DIR — системный data dir
        // тогда даже не вычисляется.
        let data_dir = self.data_dir.map(|d| d.display().to_string());
        let mut cfg = Config::from_vars(|k| match (k, &data_dir) {
            ("FOCUS_DATA_DIR", Some(d)) => Some(d.clone()),
            _ => std::env::var(k).ok(),
        })?;
        if let Some(addr) = self.addr {
            cfg.addr = addr;
        }
        Ok(cfg)
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Запустить локальный сервис (REST + SSE + MCP) на 127.0.0.1:7878.
    Serve(ConfigArgs),
    /// Показать токен доступа (создаётся при первом запуске).
    Token(ConfigArgs),
    /// Открыть desktop-клиент; если сервис не отвечает, поднять его в этом
    /// же процессе (остановится вместе с окном).
    #[cfg(feature = "ui")]
    Ui(ConfigArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => {
            init_tracing();
            let config = args.config()?;
            tokio::runtime::Runtime::new()?.block_on(focus_server::run(config))
        }
        Command::Token(args) => {
            let config = args.config()?;
            println!("{}", load_or_create_token(&config.token_path())?);
            Ok(())
        }
        #[cfg(feature = "ui")]
        Command::Ui(args) => {
            init_tracing();
            let config = args.config()?;
            ui::run(config)
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,rmcp=warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }
}

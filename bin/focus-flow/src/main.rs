//! CLI Focus Flow: `focus-flow serve` — локальный сервис, `focus-flow ui` —
//! desktop-клиент (поднимает `serve`, если тот не запущен).

use clap::{Parser, Subcommand};

/// Трекер фокуса 50/10.
#[derive(Debug, Parser)]
#[command(name = "focus-flow", version, about = "Трекер фокуса 50/10")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Запустить локальный сервис (REST + SSE + MCP) на 127.0.0.1:7878.
    Serve,
    /// Открыть desktop-клиент; при необходимости поднять сервис.
    Ui,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve => println!("focus-flow serve: сервис ещё не реализован (шаг 3 плана)"),
        Command::Ui => println!("focus-flow ui: запуск клиента ещё не реализован"),
    }
    Ok(())
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

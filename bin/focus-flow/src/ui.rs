//! `focus-flow ui`: desktop-окно и, если нужно, сервис в том же процессе.
//!
//! 1. Если на адресе уже отвечает `GET /api/health` — окно подключается к
//!    этому сервису (например, к запущенному launchd `focus-flow serve`).
//! 2. Иначе сервис стартует в отдельном потоке со своим tokio-runtime, а окно
//!    получает адрес и токен напрямую. Закрыли окно — сервис останавливается
//!    штатно (дожидается запросов, таймера и outbox, закрывает БД).
//!
//! Цикл событий окна обязан крутиться в главном потоке (macOS), поэтому сервис
//! — в фоновом.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use focus_server::{Config, load_or_create_token};
use tokio::sync::oneshot;

/// Сколько ждать ответа `/api/health` от уже запущенного сервиса.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// Сколько ждать, пока поднимется свой сервис.
const START_TIMEOUT: Duration = Duration::from_secs(15);

pub fn run(config: Config) -> anyhow::Result<()> {
    // Окружение — до появления потоков (см. `desktop::prepare_env`).
    desktop::prepare_env();
    let addr = config.addr;
    if healthy(addr) {
        tracing::info!(%addr, "сервис уже запущен — подключаюсь к нему");
        // Токен того же каталога данных, если файл есть; иначе клиент сам
        // возьмёт FOCUS_TOKEN или файл по умолчанию.
        let path = config.token_path();
        let token = std::fs::read_to_string(&path)
            .ok()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
        let opts = desktop::Options {
            addr: Some(addr.to_string()),
            token,
        };
        desktop::launch(opts, || {});
        return Ok(());
    }

    let token = load_or_create_token(&config.token_path())?;
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let service = thread::Builder::new()
        .name("focus-flow-serve".into())
        .spawn(move || -> anyhow::Result<()> {
            let rt = tokio::runtime::Runtime::new().context("не удалось создать tokio-runtime")?;
            rt.block_on(focus_server::run_until(config, async move {
                // Закрытие окна или потеря отправителя — одно и то же: стоп.
                let _ = stop_rx.await;
            }))
        })
        .context("не удалось запустить поток сервиса")?;

    let started = Instant::now();
    while !healthy(addr) {
        if service.is_finished() {
            return match service.join() {
                Ok(Err(e)) => Err(e.context("сервис не запустился")),
                Ok(Ok(())) => anyhow::bail!("сервис остановился сразу после запуска"),
                Err(_) => anyhow::bail!("поток сервиса упал при запуске"),
            };
        }
        anyhow::ensure!(
            started.elapsed() < START_TIMEOUT,
            "сервис не ответил на {addr} за {} с",
            START_TIMEOUT.as_secs()
        );
        thread::sleep(Duration::from_millis(50));
    }
    tracing::info!(%addr, "сервис запущен в процессе окна");

    let opts = desktop::Options {
        addr: Some(addr.to_string()),
        token: Some(token),
    };
    let mut stop_tx = Some(stop_tx);
    let mut service = Some(service);
    desktop::launch(opts, move || {
        tracing::info!("окно закрыто — останавливаю сервис");
        if let Some(tx) = stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = service.take() {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(error = %format!("{e:#}"), "сервис завершился с ошибкой")
                }
                Err(_) => tracing::error!("поток сервиса упал"),
            }
        }
    });
    Ok(())
}

/// Отвечает ли на `addr` сервис Focus Flow (`GET /api/health` → 200 и
/// `"status":"ok"`). Без HTTP-клиента: запрос крошечный, а тянуть reqwest
/// в бинарник ради него незачем.
fn healthy(addr: SocketAddr) -> bool {
    let probe = || -> std::io::Result<bool> {
        let mut s = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)?;
        s.set_read_timeout(Some(PROBE_TIMEOUT))?;
        s.set_write_timeout(Some(PROBE_TIMEOUT))?;
        write!(
            s,
            "GET /api/health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
        )?;
        let mut buf = Vec::new();
        s.take(4096).read_to_end(&mut buf)?;
        let text = String::from_utf8_lossy(&buf);
        Ok(text.starts_with("HTTP/1.1 200") && text.contains("\"status\":\"ok\""))
    };
    probe().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::healthy;

    #[test]
    fn nothing_listening_is_not_healthy() {
        // Порт 1 на loopback никто не слушает.
        assert!(!healthy("127.0.0.1:1".parse().unwrap()));
    }
}

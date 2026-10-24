//! Системные уведомления о конце сессии по таймеру. С фичей `notify` —
//! `notify-rust`, без неё — только запись в лог. Ошибки показа только логируются.

use std::sync::Arc;

use focus_api_types::{EndReason, ServerEvent, SessionKind};

/// Показ уведомления пользователю.
pub trait Notifier: Send + Sync {
    /// Показать уведомление. Не должен паниковать; ошибки — в лог.
    fn notify(&self, title: &str, body: &str);
}

/// Уведомления только в лог (headless, CI, тесты).
#[derive(Debug, Clone, Copy, Default)]
pub struct LogNotifier;

impl Notifier for LogNotifier {
    fn notify(&self, title: &str, body: &str) {
        tracing::info!(title, body, "уведомление");
    }
}

/// Уведомитель по умолчанию: системный при фиче `notify`, иначе — лог.
pub fn default_notifier() -> Arc<dyn Notifier> {
    #[cfg(feature = "notify")]
    {
        Arc::new(desktop::DesktopNotifier)
    }
    #[cfg(not(feature = "notify"))]
    {
        Arc::new(LogNotifier)
    }
}

/// Текст уведомления для события таймера: `(заголовок, текст)`.
/// Только для сессий, закрытых по лимиту: работа → отдых, конец отдыха.
pub fn notification_for(event: &ServerEvent, rest_min: u32) -> Option<(String, String)> {
    let ServerEvent::SessionEnded { session } = event else {
        return None;
    };
    if session.end_reason != Some(EndReason::Limit) {
        return None;
    }
    match session.kind {
        SessionKind::Work => Some((
            "Focus Flow".to_owned(),
            format!("Время работы вышло — отдохни {rest_min} минут"),
        )),
        SessionKind::Rest => Some((
            "Focus Flow".to_owned(),
            "Отдых окончен — выбери следующую задачу".to_owned(),
        )),
        SessionKind::Planning => None,
    }
}

#[cfg(feature = "notify")]
mod desktop {
    use super::Notifier;

    /// Системное уведомление через `notify-rust` (D-Bus на Linux,
    /// Notification Center на macOS) со звуком.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct DesktopNotifier;

    impl Notifier for DesktopNotifier {
        fn notify(&self, title: &str, body: &str) {
            let mut n = notify_rust::Notification::new();
            n.summary(title).body(body).appname("Focus Flow");
            #[cfg(target_os = "macos")]
            n.sound_name("default");
            #[cfg(not(target_os = "macos"))]
            n.sound_name("message-new-instant");
            if let Err(e) = n.show() {
                tracing::warn!(error = %e, title, body, "не удалось показать уведомление");
            } else {
                tracing::info!(title, body, "уведомление показано");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use focus_api_types::{SessionDto, Uuid};

    use super::*;

    fn ended(kind: SessionKind, reason: EndReason) -> ServerEvent {
        let t = Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap();
        ServerEvent::SessionEnded {
            session: SessionDto {
                id: Uuid::from_u128(1).into(),
                day_id: Uuid::from_u128(2).into(),
                kind,
                task_id: None,
                task_title: None,
                started_at: t,
                ended_at: Some(t),
                limit_sec: Some(60),
                end_reason: Some(reason),
                duration_sec: 60,
            },
        }
    }

    #[test]
    fn texts_for_limit_only() {
        let (_, body) = notification_for(&ended(SessionKind::Work, EndReason::Limit), 10).unwrap();
        assert_eq!(body, "Время работы вышло — отдохни 10 минут");
        let (_, body) = notification_for(&ended(SessionKind::Rest, EndReason::Limit), 10).unwrap();
        assert_eq!(body, "Отдых окончен — выбери следующую задачу");
        assert!(notification_for(&ended(SessionKind::Work, EndReason::Interrupted), 10).is_none());
        assert!(notification_for(&ServerEvent::Unknown, 10).is_none());
    }
}

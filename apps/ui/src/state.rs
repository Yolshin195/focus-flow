//! Общее состояние клиента: статус, дерево задач, день — и действия над ними.
//!
//! Источник истины — сервис (`FocusApi`). Клиент раз в секунду читает статус
//! и день; таймер на экране считается от `ends_at - server_time`, а не от
//! локальных счётчиков.

use std::time::Duration as StdDuration;

use dioxus::prelude::*;
use focus_api_types::{
    ActiveSession, ApiError, DayDto, DoneNext, NewTask, SessionKind, StatusDto, TaskDto, TaskId,
    TaskPatch, TaskStatus,
};

use crate::api::ApiHandle;
use crate::fmt;
use crate::platform::PlatformHandle;

/// Начальное состояние интерфейса для демо и скриншотов (`FOCUS_MOCK_SCENARIO`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiPreset {
    /// Обычный запуск.
    #[default]
    None,
    /// Открыт выбор пункта.
    Picker,
    /// Открыт диалог «Прервать».
    Interrupt,
    /// Показан выбор после «Готово».
    DoneChoice,
    /// Сразу экран «Задачи».
    Tasks,
    /// Сразу экран «День».
    Day,
}

impl UiPreset {
    /// Разбор значения `FOCUS_MOCK_SCENARIO` (лишние значения — без пресета).
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "picker" => Self::Picker,
            "interrupt" => Self::Interrupt,
            "done-choice" => Self::DoneChoice,
            "tasks" => Self::Tasks,
            "day" => Self::Day,
            _ => Self::None,
        }
    }
}

/// Состояние в контексте Dioxus. Все поля — сигналы, поэтому клон дешёвый.
#[derive(Clone, Copy)]
pub struct AppState {
    /// Последний статус от сервиса.
    pub status: Signal<Option<StatusDto>>,
    /// Сегодняшний день (сессии и итоги).
    pub day: Signal<Option<DayDto>>,
    /// Дерево задач.
    pub tasks: Signal<Vec<TaskDto>>,
    /// Последняя ошибка для тоста.
    pub error: Signal<Option<String>>,
    /// Показан выбор «Отдых / Спланировать» после нажатия «Готово».
    pub done_choice: Signal<bool>,
    /// Открыт диалог «Прервать».
    pub interrupt_dialog: Signal<bool>,
    /// Открыт выбор пункта.
    pub picker_open: Signal<bool>,
    /// Выбранный пункт (в простое — до нажатия «Начать»).
    pub selected_item: Signal<Option<TaskId>>,
    /// Пресет для демо.
    pub preset: Signal<UiPreset>,
    /// Идёт команда — кнопки неактивны.
    pub busy: Signal<bool>,
}

/// Достаёт `FocusApi` из контекста.
pub fn use_api() -> ApiHandle {
    use_context::<ApiHandle>()
}

/// Достаёт состояние из контекста.
pub fn use_app() -> AppState {
    use_context::<AppState>()
}

/// Создаёт состояние, кладёт в контекст и запускает опрос сервиса.
pub fn use_app_provider(preset: UiPreset) -> AppState {
    let state = use_context_provider(|| AppState {
        status: Signal::new(None),
        day: Signal::new(None),
        tasks: Signal::new(Vec::new()),
        error: Signal::new(None),
        done_choice: Signal::new(preset == UiPreset::DoneChoice),
        interrupt_dialog: Signal::new(preset == UiPreset::Interrupt),
        picker_open: Signal::new(preset == UiPreset::Picker),
        selected_item: Signal::new(None),
        preset: Signal::new(preset),
        busy: Signal::new(false),
    });
    let api = use_api();
    let platform = use_context::<PlatformHandle>();

    // Опрос статуса и дня раз в секунду.
    use_future(move || {
        let api = api.clone();
        let platform = platform.clone();
        async move {
            let mut state = state;
            refresh_tasks(&api, state).await;
            loop {
                let prev = state.status.peek().clone();
                match api.0.get_status().await {
                    Ok(status) => {
                        notify_transition(&platform, prev.as_ref(), &status);
                        let date = status
                            .day
                            .as_ref()
                            .map(|d| d.date)
                            .unwrap_or_else(|| fmt::local_date(status.server_time));
                        if let Ok(day) = api.0.get_day(date).await {
                            state.day.set(Some(day));
                        }
                        // Работа закончилась (не нами) — закрыть выбор и диалог.
                        if !matches!(status.active, Some(ActiveSession::Work { .. }))
                            && *state.preset.peek() == UiPreset::None
                        {
                            if *state.done_choice.peek() {
                                state.done_choice.set(false);
                            }
                            if *state.interrupt_dialog.peek() {
                                state.interrupt_dialog.set(false);
                            }
                        }
                        if prev.as_ref() != Some(&status) {
                            state.status.set(Some(status));
                        }
                    }
                    Err(e) => state.error.set(Some(e.to_string())),
                }
                futures_timer::Delay::new(StdDuration::from_secs(1)).await;
            }
        }
    });
    state
}

/// Уведомления о сменах, которые случились без участия пользователя.
fn notify_transition(platform: &PlatformHandle, prev: Option<&StatusDto>, now: &StatusDto) {
    let prev_kind = prev.and_then(|p| p.active.as_ref()).map(|a| a.kind());
    let prev_id = prev.and_then(|p| p.active.as_ref()).map(|a| a.session_id());
    let kind = now.active.as_ref().map(|a| a.kind());
    let id = now.active.as_ref().map(|a| a.session_id());
    if prev.is_none() || prev_id == id {
        return;
    }
    match (prev_kind, kind) {
        (Some(SessionKind::Work), Some(SessionKind::Rest)) => platform.0.notify(
            "50 минут прошли",
            "Сессия закрыта по лимиту. Начался отдых — встань, пройдись.",
        ),
        (Some(SessionKind::Rest), None) => platform
            .0
            .notify("Отдых окончен", "Выбери пункт и начни работу."),
        _ => {}
    }
    platform
        .0
        .keep_awake(matches!(kind, Some(SessionKind::Work)));
}

/// Перечитать дерево задач.
pub async fn refresh_tasks(api: &ApiHandle, mut state: AppState) {
    match api.0.list_tasks().await {
        Ok(tree) => state.tasks.set(tree),
        Err(e) => state.error.set(Some(e.to_string())),
    }
}

/// Перечитать статус и день сразу после команды.
async fn refresh_status(api: &ApiHandle, mut state: AppState) {
    if let Ok(status) = api.0.get_status().await {
        let date = status
            .day
            .as_ref()
            .map(|d| d.date)
            .unwrap_or_else(|| fmt::local_date(status.server_time));
        if let Ok(day) = api.0.get_day(date).await {
            state.day.set(Some(day));
        }
        state.status.set(Some(status));
    }
}

/// Текст ошибки для человека.
pub fn error_text(e: &ApiError) -> String {
    match e {
        ApiError::SessionAlreadyActive => "Сейчас уже идёт сессия — дождись её конца.".into(),
        ApiError::NoActiveSession => "Сессия уже закончилась.".into(),
        ApiError::ItemRequired => "В работу берётся пункт, а не задача целиком.".into(),
        ApiError::ItemNotFound => "Пункт не найден — список обновлён.".into(),
        ApiError::DayClosed => "День закрыт. Открой его снова, чтобы продолжить.".into(),
        ApiError::Validation(m) => format!("Проверь ввод: {m}."),
        ApiError::PluginUnavailable(m) => format!("Импорт недоступен: {m}."),
        ApiError::Transport(_) => "Нет связи с сервисом Focus Flow.".into(),
        other => other.to_string(),
    }
}

/// Команды интерфейса. Каждая запускает задачу и после неё обновляет данные.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Начать работу над пунктом.
    StartWork(TaskId),
    /// Начать планирование.
    StartPlanning,
    /// «Готово» и что дальше.
    Done(DoneNext),
    /// «Прервать».
    Interrupt,
    /// «Стоп» планирования.
    Stop,
    /// Создать задачу или пункт.
    Create(NewTask),
    /// Отметить готовым / вернуть в план.
    SetDone(TaskId, bool),
    /// Импорт по ссылке.
    Import(String),
}

/// Выполнить команду. `on_ok` вызывается после успеха.
pub fn run(api: ApiHandle, mut state: AppState, cmd: Command, on_ok: impl FnOnce() + 'static) {
    spawn(async move {
        state.busy.set(true);
        let tasks_changed = matches!(
            cmd,
            Command::Create(_)
                | Command::SetDone(..)
                | Command::Import(_)
                | Command::Done(_)
                | Command::StartWork(_)
        );
        let result: Result<(), ApiError> = match cmd {
            Command::StartWork(id) => api.0.start_work(id).await.map(drop),
            Command::StartPlanning => api.0.start_planning().await.map(drop),
            Command::Done(next) => api.0.done(next).await.map(drop),
            Command::Interrupt => api.0.interrupt().await.map(drop),
            Command::Stop => api.0.stop().await.map(drop),
            Command::Create(t) => api.0.create_task(t).await.map(drop),
            Command::SetDone(id, done) => {
                let status = if done {
                    TaskStatus::Done
                } else {
                    TaskStatus::Planned
                };
                let patch = TaskPatch {
                    status: Some(status),
                    ..TaskPatch::default()
                };
                api.0.update_task(id, patch).await.map(drop)
            }
            Command::Import(url) => api.0.import(url).await.map(drop),
        };
        match result {
            Ok(()) => {
                state.error.set(None);
                on_ok();
            }
            Err(e) => state.error.set(Some(error_text(&e))),
        }
        refresh_status(&api, state).await;
        if tasks_changed {
            refresh_tasks(&api, state).await;
        }
        state.busy.set(false);
    });
}

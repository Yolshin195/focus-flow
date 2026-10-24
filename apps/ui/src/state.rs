//! Общее состояние клиента: статус, дерево задач, день — и действия над ними.
//!
//! Источник истины — сервис (`FocusApi`). С настоящим сервисом данные
//! приходят событиями SSE (`/api/events`) плюс страховочный опрос раз в 15 с;
//! с моком — опрос раз в секунду. Между событиями клиент сам досчитывает
//! время: таймер — от `ends_at` и `server_time` последнего снимка плюс
//! прошедшее по местным часам (смещение часов клиента не важно).

use std::time::Duration as StdDuration;

use dioxus::prelude::*;
use focus_api_types::chrono::{DateTime, Duration, Utc};
use focus_api_types::{
    ActiveSession, ApiError, DayDto, DoneNext, NewTask, ServerEvent, SessionKind, StatusDto,
    TaskDto, TaskId, TaskPatch, TaskStatus,
};
use futures_util::future::{select, Either};
use futures_util::StreamExt;

use crate::api::ApiHandle;
use crate::fmt;
use crate::platform::PlatformHandle;
use crate::timer::TimerView;

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
    /// Сразу экран «Настройки».
    Settings,
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
            "settings" => Self::Settings,
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
    /// Связь с сервисом.
    pub conn: Signal<Conn>,
    /// Сколько раз извне окна (трей) попросили открыть «Фокус» — см. [`AppState::reveal`].
    pub(crate) focus_requests: Signal<u64>,
    /// Последний статус от сервиса как есть (`status` — он же, досчитанный).
    pub(crate) base_status: Signal<Option<Stamped<StatusDto>>>,
    /// Последний день от сервиса как есть.
    pub(crate) base_day: Signal<Option<Stamped<DayDto>>>,
    /// Досчитывать время между снимками (у сервиса); мок опрашивается
    /// каждую секунду и может стоять на месте (`FOCUS_MOCK_SPEED=0`).
    local_clock: bool,
}

/// Что открыть на экране «Фокус» по запросу извне окна (трей, строка меню).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    /// Только экран «Фокус».
    Focus,
    /// «Фокус» с предупреждением «Прервать работу?» — прервать можно только
    /// из окна, через это предупреждение.
    Interrupt,
    /// «Фокус» с выбором пункта — без пункта работа не стартует.
    StartWork,
}

impl AppState {
    /// Открыть «Фокус» и, если фаза подходит, диалог. Окно показывает сама
    /// платформа; переход на экран делает layout.
    pub fn reveal(mut self, what: Reveal) {
        *self.focus_requests.write() += 1;
        let view = TimerView::of(self.status.peek().as_ref());
        match (what, view) {
            (Reveal::Interrupt, TimerView::Work { .. }) => {
                self.done_choice.set(false);
                self.interrupt_dialog.set(true);
            }
            (Reveal::StartWork, TimerView::Idle | TimerView::Planning { .. }) => {
                self.picker_open.set(true);
            }
            _ => {}
        }
    }
}

/// Достаёт `FocusApi` из контекста.
pub fn use_api() -> ApiHandle {
    use_context::<ApiHandle>()
}

/// Достаёт состояние из контекста.
pub fn use_app() -> AppState {
    use_context::<AppState>()
}

/// Связь с сервисом — для плашки «Нет связи с сервисом».
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Conn {
    /// Ещё не знаем (первый запрос в пути).
    #[default]
    Unknown,
    /// Данные приходят.
    Online,
    /// Сервис недоступен (сеть, сервис не запущен).
    Offline,
    /// Сервис отклонил токен.
    Unauthorized,
}

/// Снимок от сервиса и момент получения по местным часам: от него экран
/// досчитывает время сам, между событиями.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Stamped<T> {
    pub(crate) value: T,
    pub(crate) at: DateTime<Utc>,
}

impl<T> Stamped<T> {
    fn now(value: T) -> Self {
        Self {
            value,
            at: Utc::now(),
        }
    }
}

/// Раз во сколько опрашивать мок (у него нет событий).
const MOCK_POLL: StdDuration = StdDuration::from_secs(1);
/// Страховочный опрос при живом SSE: сон системы, потерянные события.
const SAFETY_POLL: StdDuration = StdDuration::from_secs(15);
/// Нет ни данных, ни keep-alive дольше этого — соединение SSE мёртвое.
const SSE_STALL: StdDuration = StdDuration::from_secs(40);
/// Задержки переподключения SSE.
const BACKOFF_MIN_MS: u64 = 500;
const BACKOFF_MAX_MS: u64 = 5_000;

/// Создаёт состояние, кладёт в контекст и запускает получение данных:
/// для сервиса — SSE + страховочный опрос, для мока — опрос раз в секунду.
/// Отдельный таймер раз в секунду досчитывает таймер и норму локально.
pub fn use_app_provider(preset: UiPreset) -> AppState {
    let api = use_api();
    let local_clock = api.is_remote();
    let state = use_context_provider(|| AppState {
        status: root_signal(None),
        day: root_signal(None),
        tasks: root_signal(Vec::new()),
        error: root_signal(None),
        done_choice: root_signal(preset == UiPreset::DoneChoice),
        interrupt_dialog: root_signal(preset == UiPreset::Interrupt),
        picker_open: root_signal(preset == UiPreset::Picker),
        selected_item: root_signal(None),
        preset: root_signal(preset),
        busy: root_signal(false),
        conn: root_signal(Conn::Unknown),
        focus_requests: root_signal(0),
        base_status: root_signal(None),
        base_day: root_signal(None),
        local_clock,
    });
    let platform = use_context::<PlatformHandle>();

    // Опрос: у мока — главный источник, у сервиса — страховка.
    let poll_api = api.clone();
    let poll_platform = platform.clone();
    use_future(move || {
        let api = poll_api.clone();
        let platform = poll_platform.clone();
        async move {
            let period = if api.is_remote() {
                SAFETY_POLL
            } else {
                MOCK_POLL
            };
            refresh_tasks(&api, state).await;
            loop {
                poll_once(&api, &platform, state).await;
                futures_timer::Delay::new(period).await;
            }
        }
    });

    // Живые обновления по SSE.
    let sse_api = api.clone();
    let sse_platform = platform.clone();
    use_future(move || {
        let api = sse_api.clone();
        let platform = sse_platform.clone();
        async move {
            if api.is_remote() {
                sse_loop(&api, &platform, state).await;
            }
        }
    });

    // Локальный ход времени: раз в секунду (по секундам часов сервиса)
    // пересчитать показываемые статус и день из последних снимков.
    use_future(move || async move {
        loop {
            let offset_ms = state
                .base_status
                .peek()
                .as_ref()
                .map(|b| (b.value.server_time - b.at).num_milliseconds())
                .unwrap_or(0);
            let server_ms = (Utc::now().timestamp_millis() + offset_ms).rem_euclid(1000);
            let wait = (1000 - server_ms) as u64 + 5;
            futures_timer::Delay::new(StdDuration::from_millis(wait)).await;
            derive(state);
        }
    });
    state
}

/// Сигнал состояния с владельцем — корневым scope. Команды выполняются в
/// `spawn_forever`, то есть тоже в корне: сигнал, которым владеет `App`,
/// оттуда читался бы «выше» владельца, и Dioxus писал бы в лог
/// предупреждение `copy_value_hoisted` на каждое чтение и запись.
#[track_caller]
fn root_signal<T: 'static>(value: T) -> Signal<T> {
    Signal::new_in_scope(value, ScopeId::ROOT)
}

/// Один опрос статуса и дня. Ошибку связи показывает плашка, а не тост.
async fn poll_once(api: &ApiHandle, platform: &PlatformHandle, mut state: AppState) {
    match api.0.get_status().await {
        Ok(status) => {
            mark(state, Conn::Online);
            apply_status(api, platform, state, status).await;
        }
        Err(e) => {
            if !conn_error(state, &e) {
                state.error.set(Some(error_text(&e)));
            }
        }
    }
}

/// Ошибка связи или доступа → плашка; `true`, если ошибка такая.
fn conn_error(state: AppState, e: &ApiError) -> bool {
    match e {
        ApiError::Transport(_) => mark(state, Conn::Offline),
        ApiError::Unauthorized => mark(state, Conn::Unauthorized),
        _ => return false,
    }
    true
}

fn mark(mut state: AppState, conn: Conn) {
    if *state.conn.peek() != conn {
        state.conn.set(conn);
    }
}

/// Подписка на `/api/events` с переподключением. Первым событием сервис
/// присылает текущий статус, поэтому после переподключения ничего не
/// теряется; пропущенные `task_changed` закрывает перечитывание задач.
async fn sse_loop(api: &ApiHandle, platform: &PlatformHandle, state: AppState) {
    let Some(http) = api.events() else { return };
    let mut backoff = BACKOFF_MIN_MS;
    let mut first = true;
    loop {
        match http.events().await {
            Ok(stream) => {
                mark(state, Conn::Online);
                backoff = BACKOFF_MIN_MS;
                if !std::mem::take(&mut first) {
                    refresh_tasks(api, state).await;
                }
                let mut stream = std::pin::pin!(stream);
                loop {
                    let next = stream.next();
                    let stall = futures_timer::Delay::new(SSE_STALL);
                    let item = match select(next, stall).await {
                        Either::Left((Some(item), _)) => item,
                        // Поток закрыт или молчит дольше keep-alive.
                        Either::Left((None, _)) | Either::Right(_) => break,
                    };
                    let Ok(events) = item else { break };
                    for ev in events {
                        on_event(api, platform, state, ev).await;
                    }
                }
            }
            Err(e) => {
                conn_error(state, &e);
            }
        }
        futures_timer::Delay::new(StdDuration::from_millis(backoff)).await;
        backoff = (backoff * 2).min(BACKOFF_MAX_MS);
        // Пока ждали — проверить сервис обычным запросом (обновит плашку).
        poll_once(api, platform, state).await;
    }
}

async fn on_event(api: &ApiHandle, platform: &PlatformHandle, state: AppState, ev: ServerEvent) {
    match ev {
        ServerEvent::StatusChanged(status) => apply_status(api, platform, state, status).await,
        ServerEvent::TaskChanged { .. } => refresh_tasks(api, state).await,
        // За `session_ended` сервис сразу шлёт `status_changed`; уведомление
        // о лимите показывает сам сервис.
        ServerEvent::SessionEnded { .. }
        | ServerEvent::PluginError { .. }
        | ServerEvent::Unknown => {}
    }
}

/// Принять статус от сервиса: снимок, день, уведомления, закрытие диалогов.
/// Более старый статус (ответ опроса, обогнанный событием) не применяется.
async fn apply_status(
    api: &ApiHandle,
    platform: &PlatformHandle,
    mut state: AppState,
    status: StatusDto,
) {
    let prev = state.base_status.peek().as_ref().map(|b| b.value.clone());
    if prev
        .as_ref()
        .is_some_and(|p| p.server_time > status.server_time)
    {
        return;
    }
    // Уведомления о лимите показывает сервис — клиент сервиса их не дублирует.
    if !api.is_remote() {
        notify_transition(platform, prev.as_ref(), &status);
    }
    let kind = status.active.as_ref().map(|a| a.kind());
    if prev
        .as_ref()
        .and_then(|p| p.active.as_ref())
        .map(|a| a.kind())
        != kind
        || prev.is_none()
    {
        platform
            .0
            .keep_awake(matches!(kind, Some(SessionKind::Work)));
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
    let date = status
        .day
        .as_ref()
        .map(|d| d.date)
        .unwrap_or_else(|| fmt::local_date(status.server_time));
    state.base_status.set(Some(Stamped::now(status)));
    match api.0.get_day(date).await {
        Ok(day) => state.base_day.set(Some(Stamped::now(day))),
        Err(e) => {
            conn_error(state, &e);
        }
    }
    derive(state);
}

/// Показываемые статус и день = последние снимки + прошедшее с их получения.
fn derive(mut state: AppState) {
    let now = Utc::now();
    let elapsed = |at: DateTime<Utc>| {
        if state.local_clock {
            now - at
        } else {
            Duration::zero()
        }
    };
    let status = state
        .base_status
        .peek()
        .as_ref()
        .map(|b| advance_status(&b.value, elapsed(b.at)));
    let day = state
        .base_day
        .peek()
        .as_ref()
        .map(|b| advance_day(&b.value, elapsed(b.at)));
    if *state.status.peek() != status {
        state.status.set(status);
    }
    if *state.day.peek() != day {
        state.day.set(day);
    }
}

/// Статус через `dt`: время сервиса и отработанное (если идёт работа или
/// планирование) сдвигаются; смену сессии пришлёт сервис.
pub(crate) fn advance_status(s: &StatusDto, dt: Duration) -> StatusDto {
    let dt = dt.max(Duration::zero());
    let mut s = s.clone();
    s.server_time += dt;
    if matches!(
        s.active.as_ref().map(|a| a.kind()),
        Some(SessionKind::Work | SessionKind::Planning)
    ) {
        s.worked_today_sec += dt.num_seconds();
    }
    s
}

/// День через `dt`: растут длительность активной сессии и итоги её вида.
pub(crate) fn advance_day(d: &DayDto, dt: Duration) -> DayDto {
    let sec = dt.max(Duration::zero()).num_seconds();
    let mut d = d.clone();
    if sec == 0 {
        return d;
    }
    if let Some(active) = d.sessions.iter_mut().find(|s| s.ended_at.is_none()) {
        active.duration_sec += sec;
        match active.kind {
            SessionKind::Work => {
                d.totals.work_sec += sec;
                d.totals.worked_sec += sec;
            }
            SessionKind::Planning => {
                d.totals.planning_sec += sec;
                d.totals.worked_sec += sec;
            }
            SessionKind::Rest => d.totals.rest_sec += sec,
        }
    }
    d
}

/// Уведомления о сменах, которые случились без участия пользователя
/// (только для мока: сервис уведомляет сам).
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
}

/// Перечитать дерево задач.
pub async fn refresh_tasks(api: &ApiHandle, mut state: AppState) {
    match api.0.list_tasks().await {
        Ok(tree) => {
            if *state.tasks.peek() != tree {
                state.tasks.set(tree);
            }
        }
        Err(e) => {
            if !conn_error(state, &e) {
                state.error.set(Some(error_text(&e)));
            }
        }
    }
}

/// Перечитать статус и день сразу после команды (не дожидаясь события).
async fn refresh_status(api: &ApiHandle, platform: &PlatformHandle, state: AppState) {
    poll_once(api, platform, state).await;
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
    /// Отдых (после планирования).
    StartRest,
    /// Создать задачу или пункт.
    Create(NewTask),
    /// Отметить готовым / вернуть в план.
    SetDone(TaskId, bool),
    /// Импорт по ссылке.
    Import(String),
}

/// Выполнить команду. `on_ok` вызывается после успеха.
pub fn run(api: ApiHandle, mut state: AppState, cmd: Command, on_ok: impl FnOnce() + 'static) {
    let platform = consume_context::<PlatformHandle>();
    // Не `spawn`: задача привязалась бы к компоненту, который вызвал команду,
    // а это часто диалог, который сам закрывается этой же командой (выбор
    // пункта, «Готово», «Прервать»), — и запрос к сервису отменился бы на лету.
    dioxus::core::spawn_forever(async move {
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
            Command::StartRest => api.0.start_rest().await.map(drop),
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
            Err(e) => {
                conn_error(state, &e);
                state.error.set(Some(error_text(&e)));
            }
        }
        refresh_status(&api, &platform, state).await;
        if tasks_changed {
            refresh_tasks(&api, state).await;
        }
        state.busy.set(false);
    });
}

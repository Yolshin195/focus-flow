//! Общие помощники сценариев.

use std::fmt::Debug;

use chrono::Duration;
use focus_api_types::{
    ActiveSession, ApiError, FocusApi, NewTask, SessionKind, StatusDto, TaskDto, TaskId,
};

/// Управляемые часы бэкенда (фейковый `Clock` в моке или в тестах сервиса).
pub trait TestClock {
    /// Сдвинуть «сейчас» бэкенда вперёд.
    fn advance(&self, by: Duration);
}

/// Результат шага сценария.
pub type Step<T = ()> = Result<T, String>;

/// Оборачивает ошибку API в текст с контекстом.
pub fn call<T>(what: &str, r: Result<T, ApiError>) -> Step<T> {
    r.map_err(|e| format!("{what}: неожиданная ошибка {e:?} ({e})"))
}

/// Ждёт конкретную ошибку.
pub fn expect_err<T: Debug>(what: &str, r: Result<T, ApiError>, expected: &ApiError) -> Step {
    match r {
        Err(e) if &e == expected => Ok(()),
        Err(e) => Err(format!(
            "{what}: ожидалась ошибка {expected:?}, получена {e:?}"
        )),
        Ok(v) => Err(format!(
            "{what}: ожидалась ошибка {expected:?}, а вызов прошёл: {v:?}"
        )),
    }
}

/// Проверка условия с сообщением.
pub fn ensure(cond: bool, msg: impl FnOnce() -> String) -> Step {
    if cond { Ok(()) } else { Err(msg()) }
}

/// Сравнение с понятным сообщением.
pub fn ensure_eq<T: PartialEq + Debug>(what: &str, actual: T, expected: T) -> Step {
    ensure(actual == expected, || {
        format!("{what}: ожидалось {expected:?}, получено {actual:?}")
    })
}

/// Завершает активную работу («Прервать») или планирование («Стоп»).
/// Активный отдых пропустить нельзя — это ошибка порядка сценариев.
pub async fn ensure_idle(api: &dyn FocusApi) -> Step {
    let status = call("get_status", api.get_status().await)?;
    match status.active {
        None => Ok(()),
        Some(ActiveSession::Work { .. }) => call("interrupt", api.interrupt().await).map(drop),
        Some(ActiveSession::Planning { .. }) => call("stop", api.stop().await).map(drop),
        Some(ActiveSession::Rest { .. }) => Err(
            "идёт отдых, а его нельзя пропустить: сценарий требует свежий бэкенд \
             или должен идти после всех остальных"
                .into(),
        ),
    }
}

/// Задача верхнего уровня с одним пунктом; возвращает `(задача, пункт)`.
pub async fn task_with_item(api: &dyn FocusApi, name: &str) -> Step<(TaskDto, TaskDto)> {
    let parent = call(
        "create_task(задача)",
        api.create_task(NewTask::new(format!("контракт: {name}")))
            .await,
    )?;
    let item = call(
        "create_task(пункт)",
        api.create_task(NewTask::child(parent.id, format!("пункт: {name}")))
            .await,
    )?;
    ensure_eq(
        "parent_id созданного пункта",
        item.parent_id,
        Some(parent.id),
    )?;
    Ok((parent, item))
}

/// Ищет узел в дереве `list_tasks`.
pub async fn find_task(api: &dyn FocusApi, id: TaskId) -> Step<TaskDto> {
    let tree = call("list_tasks", api.list_tasks().await)?;
    tree.iter()
        .find_map(|t| t.find(id))
        .cloned()
        .ok_or_else(|| format!("узел {id} не найден в list_tasks"))
}

/// Длительности из настроек, в секундах: `(work, rest)`.
pub async fn limits_sec(api: &dyn FocusApi) -> Step<(i64, i64)> {
    let s = call("get_settings", api.get_settings().await)?;
    Ok((i64::from(s.work_min) * 60, i64::from(s.rest_min) * 60))
}

/// Ждёт активную сессию нужного вида.
pub fn expect_active(what: &str, status: &StatusDto, kind: SessionKind) -> Step<ActiveSession> {
    match &status.active {
        Some(a) if a.kind() == kind => Ok(a.clone()),
        other => Err(format!(
            "{what}: ожидалась активная сессия {kind:?}, получено {other:?}"
        )),
    }
}

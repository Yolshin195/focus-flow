//! Сценарии. Каждый независим: начинает с [`ensure_idle`] и создаёт свои задачи.

use chrono::Duration;
use focus_api_types::{
    ActiveSession, ApiError, DoneNext, EndReason, FocusApi, NewTask, SessionKind, TaskId,
    TaskStatus, Uuid,
};

use crate::helpers::{
    Step, TestClock, call, ensure, ensure_eq, ensure_idle, expect_active, expect_err, find_task,
    limits_sec, task_with_item,
};

/// Старт работы автоматически открывает день; активна `Work` с этим пунктом
/// и `ends_at = started_at + work_min`.
pub async fn start_work_opens_day(api: &dyn FocusApi) -> Step {
    ensure_idle(api).await?;
    let (work_sec, _) = limits_sec(api).await?;
    let (_, item) = task_with_item(api, "автооткрытие дня").await?;

    let status = call("start_work", api.start_work(item.id).await)?;
    let day = status
        .day
        .as_ref()
        .ok_or("после start_work в статусе нет дня — день не открылся")?;
    ensure(day.closed_at.is_none(), || {
        format!("после start_work день закрыт: {day:?}")
    })?;
    ensure(day.opened_at <= status.server_time, || {
        format!(
            "день открыт в будущем: {day:?}, server_time {}",
            status.server_time
        )
    })?;

    match expect_active("start_work", &status, SessionKind::Work)? {
        ActiveSession::Work {
            item: active_item,
            started_at,
            ends_at,
            ..
        } => {
            ensure_eq("пункт активной работы", active_item.id, item.id)?;
            ensure_eq("заголовок пункта", active_item.title, item.title.clone())?;
            ensure_eq(
                "ends_at - started_at",
                (ends_at - started_at).num_seconds(),
                work_sec,
            )?;
        }
        other => return Err(format!("ожидалась Work, получено {other:?}")),
    }

    let again = call("get_status", api.get_status().await)?;
    ensure_eq("active в get_status", again.active, status.active)?;
    ensure_idle(api).await
}

/// Старт работы с несуществующим id → `ItemNotFound`, сессия не стартует.
pub async fn start_work_unknown_item_not_found(api: &dyn FocusApi) -> Step {
    ensure_idle(api).await?;
    let missing = TaskId::from(Uuid::from_u128(0xdead_beef_dead_beef_dead_beef_dead_beef));
    expect_err(
        "start_work(несуществующий)",
        api.start_work(missing).await,
        &ApiError::ItemNotFound,
    )?;
    let status = call("get_status", api.get_status().await)?;
    ensure_eq("active после ошибки", status.active, None)
}

/// В работу берётся только пункт (узел без детей): задача с пунктами →
/// `ItemRequired`. Пункт без родителя — допустим.
pub async fn start_work_on_task_with_children_requires_item(api: &dyn FocusApi) -> Step {
    ensure_idle(api).await?;
    let (parent, _) = task_with_item(api, "только пункт").await?;
    expect_err(
        "start_work(задача с пунктами)",
        api.start_work(parent.id).await,
        &ApiError::ItemRequired,
    )?;
    let status = call("get_status", api.get_status().await)?;
    ensure_eq("active после ItemRequired", status.active, None)?;

    let lone = call(
        "create_task(пункт без родителя)",
        api.create_task(NewTask::new("контракт: пункт без родителя"))
            .await,
    )?;
    let status = call(
        "start_work(пункт без родителя)",
        api.start_work(lone.id).await,
    )?;
    expect_active("start_work(пункт без родителя)", &status, SessionKind::Work)?;
    ensure_idle(api).await
}

/// Повторный старт при активной работе → `SessionAlreadyActive`; «Стоп»
/// во время работы → `WrongSessionKind`. Активная сессия не меняется.
pub async fn second_start_while_active_conflicts(api: &dyn FocusApi) -> Step {
    ensure_idle(api).await?;
    let (parent, first) = task_with_item(api, "повторный старт").await?;
    let second = call(
        "create_task(второй пункт)",
        api.create_task(NewTask::child(parent.id, "второй пункт"))
            .await,
    )?;

    let status = call("start_work", api.start_work(first.id).await)?;
    let active = expect_active("start_work", &status, SessionKind::Work)?;

    expect_err(
        "второй start_work",
        api.start_work(second.id).await,
        &ApiError::SessionAlreadyActive,
    )?;
    expect_err(
        "start_planning во время работы",
        api.start_planning().await,
        &ApiError::SessionAlreadyActive,
    )?;
    expect_err(
        "start_rest во время работы",
        api.start_rest().await,
        &ApiError::SessionAlreadyActive,
    )?;
    expect_err(
        "stop во время работы",
        api.stop().await,
        &ApiError::WrongSessionKind {
            expected: SessionKind::Planning,
            actual: SessionKind::Work,
        },
    )?;

    let after = call("get_status", api.get_status().await)?;
    ensure_eq("активная сессия после отказов", after.active, Some(active))?;
    ensure_idle(api).await
}

/// «Прервать» закрывает работу с `interrupted`; пункт не становится `done`;
/// активной сессии нет; сессия видна в дне.
pub async fn interrupt_ends_work_as_interrupted(api: &dyn FocusApi) -> Step {
    ensure_idle(api).await?;
    let (_, item) = task_with_item(api, "прервать").await?;
    let started = call("start_work", api.start_work(item.id).await)?;
    let session_id = expect_active("start_work", &started, SessionKind::Work)?.session_id();

    let res = call("interrupt", api.interrupt().await)?;
    ensure_eq("id закрытой сессии", res.ended.id, session_id)?;
    ensure_eq("kind", res.ended.kind, SessionKind::Work)?;
    ensure_eq(
        "end_reason",
        res.ended.end_reason,
        Some(EndReason::Interrupted),
    )?;
    ensure_eq("task_id", res.ended.task_id, Some(item.id))?;
    ensure(res.ended.ended_at.is_some(), || {
        format!("у прерванной сессии нет ended_at: {:?}", res.ended)
    })?;
    ensure_eq("active после interrupt", res.status.active, None)?;

    let item_after = find_task(api, item.id).await?;
    ensure(item_after.status != TaskStatus::Done, || {
        format!("после interrupt пункт стал done: {item_after:?}")
    })?;

    let date = res
        .status
        .day
        .as_ref()
        .ok_or("после interrupt нет дня")?
        .date;
    let day = call("get_day", api.get_day(date).await)?;
    let in_day = day
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| format!("прерванной сессии {session_id} нет в get_day({date})"))?;
    ensure_eq(
        "end_reason в get_day",
        in_day.end_reason,
        Some(EndReason::Interrupted),
    )
}

/// Без активной сессии «Готово», «Прервать» и «Стоп» → `NoActiveSession`.
pub async fn session_end_without_active_fails(api: &dyn FocusApi) -> Step {
    ensure_idle(api).await?;
    expect_err(
        "done без сессии",
        api.done(DoneNext::Rest).await,
        &ApiError::NoActiveSession,
    )?;
    expect_err(
        "interrupt без сессии",
        api.interrupt().await,
        &ApiError::NoActiveSession,
    )?;
    expect_err(
        "stop без сессии",
        api.stop().await,
        &ApiError::NoActiveSession,
    )
}

/// Закрытый день не даёт стартовать сессии (`DayClosed`); `open_day`
/// открывает тот же день снова.
pub async fn closed_day_blocks_sessions(api: &dyn FocusApi) -> Step {
    ensure_idle(api).await?;
    let (_, item) = task_with_item(api, "закрытый день").await?;
    let opened = call("open_day", api.open_day().await)?;
    let day = opened.day.clone().ok_or("после open_day нет дня")?;
    ensure(day.closed_at.is_none(), || {
        format!("после open_day день закрыт: {day:?}")
    })?;

    let closed = call("close_day", api.close_day().await)?;
    let closed_day = closed.day.clone().ok_or("после close_day нет дня")?;
    ensure(closed_day.closed_at.is_some(), || {
        format!("после close_day нет closed_at: {closed_day:?}")
    })?;

    expect_err(
        "start_work в закрытый день",
        api.start_work(item.id).await,
        &ApiError::DayClosed,
    )?;
    expect_err(
        "start_planning в закрытый день",
        api.start_planning().await,
        &ApiError::DayClosed,
    )?;

    let reopened = call("open_day (повторно)", api.open_day().await)?;
    let reopened_day = reopened.day.ok_or("после повторного open_day нет дня")?;
    ensure_eq("id дня после повторного открытия", reopened_day.id, day.id)?;
    ensure_eq(
        "closed_at после повторного открытия",
        reopened_day.closed_at,
        None,
    )
}

/// «Готово» с `next = planning`: работа закрыта с `task_done`, пункт `done`,
/// сразу идёт планирование.
pub async fn done_then_planning(api: &dyn FocusApi) -> Step {
    ensure_idle(api).await?;
    let (_, item) = task_with_item(api, "готово → планирование").await?;
    call("start_work", api.start_work(item.id).await)?;

    let res = call("done(planning)", api.done(DoneNext::Planning).await)?;
    ensure_eq(
        "end_reason",
        res.ended.end_reason,
        Some(EndReason::TaskDone),
    )?;
    ensure_eq("kind закрытой", res.ended.kind, SessionKind::Work)?;
    let planning = expect_active("после done(planning)", &res.status, SessionKind::Planning)?;
    ensure_eq(
        "планирование стартует в момент конца работы",
        Some(planning.started_at()),
        res.ended.ended_at,
    )?;

    let item_after = find_task(api, item.id).await?;
    ensure_eq("статус пункта", item_after.status, TaskStatus::Done)?;
    ensure(item_after.done_at.is_some(), || {
        format!("у done-пункта нет done_at: {item_after:?}")
    })?;
    ensure_idle(api).await
}

/// «Стоп» планирования → `stopped`; время планирования идёт в норму
/// (`worked_today_sec` и `totals.worked_sec`).
pub async fn planning_counts_in_norm(api: &dyn FocusApi, clock: Option<&dyn TestClock>) -> Step {
    ensure_idle(api).await?;
    let before = call("get_status", api.get_status().await)?.worked_today_sec;

    let started = call("start_planning", api.start_planning().await)?;
    let planning = expect_active("start_planning", &started, SessionKind::Planning)?;
    ensure_eq("ends_at у планирования", planning.ends_at(), None)?;
    expect_err(
        "interrupt во время планирования",
        api.interrupt().await,
        &ApiError::WrongSessionKind {
            expected: SessionKind::Work,
            actual: SessionKind::Planning,
        },
    )?;

    if let Some(clock) = clock {
        clock.advance(Duration::seconds(120));
    }
    let res = call("stop", api.stop().await)?;
    ensure_eq("id закрытой", res.ended.id, planning.session_id())?;
    ensure_eq("kind закрытой", res.ended.kind, SessionKind::Planning)?;
    ensure_eq("end_reason", res.ended.end_reason, Some(EndReason::Stopped))?;
    ensure_eq("limit_sec у планирования", res.ended.limit_sec, None)?;
    if clock.is_some() {
        ensure_eq("длительность планирования", res.ended.duration_sec, 120)?;
    }
    ensure_eq("active после stop", res.status.active.clone(), None)?;
    ensure_eq(
        "worked_today_sec: прирост = длительности планирования",
        res.status.worked_today_sec - before,
        res.ended.duration_sec,
    )?;

    let date = res.status.day.as_ref().ok_or("после stop нет дня")?.date;
    let day = call("get_day", api.get_day(date).await)?;
    let t = day.totals;
    ensure_eq(
        "totals.worked_sec = work + planning",
        t.worked_sec,
        t.work_sec + t.planning_sec,
    )?;
    ensure_eq(
        "totals.worked_sec = worked_today_sec",
        t.worked_sec,
        res.status.worked_today_sec,
    )?;
    ensure(t.planning_sec >= res.ended.duration_sec, || {
        format!("totals.planning_sec меньше закрытого планирования: {t:?}")
    })
}

/// «Готово» с `next = rest`: пункт `done`, сразу отдых длиной `rest_min`;
/// время отдыха в норму не идёт. Оставляет активный отдых — запускать последним.
pub async fn done_then_rest_rest_not_in_norm(
    api: &dyn FocusApi,
    clock: Option<&dyn TestClock>,
) -> Step {
    ensure_idle(api).await?;
    let (_, rest_sec) = limits_sec(api).await?;
    let (_, item) = task_with_item(api, "готово → отдых").await?;
    call("start_work", api.start_work(item.id).await)?;
    if let Some(clock) = clock {
        clock.advance(Duration::seconds(60));
    }

    let res = call("done(rest)", api.done(DoneNext::Rest).await)?;
    ensure_eq(
        "end_reason",
        res.ended.end_reason,
        Some(EndReason::TaskDone),
    )?;
    if clock.is_some() {
        ensure_eq("длительность работы", res.ended.duration_sec, 60)?;
    }
    let rest = expect_active("после done(rest)", &res.status, SessionKind::Rest)?;
    ensure_eq(
        "отдых стартует в момент конца работы",
        Some(rest.started_at()),
        res.ended.ended_at,
    )?;
    let ends_at = rest.ends_at().ok_or("у отдыха нет ends_at")?;
    ensure_eq(
        "ends_at - started_at у отдыха",
        (ends_at - rest.started_at()).num_seconds(),
        rest_sec,
    )?;

    let item_after = find_task(api, item.id).await?;
    ensure_eq("статус пункта", item_after.status, TaskStatus::Done)?;

    expect_err(
        "start_work во время отдыха",
        api.start_work(item.id).await,
        &ApiError::SessionAlreadyActive,
    )?;
    expect_err(
        "interrupt во время отдыха",
        api.interrupt().await,
        &ApiError::WrongSessionKind {
            expected: SessionKind::Work,
            actual: SessionKind::Rest,
        },
    )?;

    let worked = res.status.worked_today_sec;
    if let Some(clock) = clock {
        clock.advance(Duration::seconds(300));
    }
    let later = call("get_status", api.get_status().await)?;
    expect_active("через время отдыха", &later, SessionKind::Rest)?;
    ensure_eq(
        "worked_today_sec во время отдыха не растёт",
        later.worked_today_sec,
        worked,
    )?;

    let date = later.day.as_ref().ok_or("во время отдыха нет дня")?.date;
    let day = call("get_day", api.get_day(date).await)?;
    ensure(
        day.sessions
            .iter()
            .any(|s| s.id == rest.session_id() && s.kind == SessionKind::Rest),
        || format!("активного отдыха нет в get_day({date})"),
    )?;
    ensure_eq(
        "totals.worked_sec не включает отдых",
        day.totals.worked_sec,
        day.totals.work_sec + day.totals.planning_sec,
    )?;
    if clock.is_some() {
        ensure_eq("totals.rest_sec", day.totals.rest_sec, 300)?;
    }
    Ok(())
}

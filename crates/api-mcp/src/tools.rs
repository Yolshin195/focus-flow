//! Инструменты ядра: по одному на команду из SPEC «MCP-инструменты».
//! Описания — для LLM, по-русски и коротко.

use std::future::Future;
use std::sync::Arc;

use chrono::NaiveDate;
use focus_api_types::{
    ApiError, DoneNext, NewTask, ServerEvent, SessionEndResult, StatusDto, TaskId, TaskPatch,
};
use focus_core::FocusService;
use focus_plugin_api::{McpTool, ToolOutput};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::McpState;
use crate::fmt;

const NO_PAUSE: &str = "Паузы нет.";

fn schema(props: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

fn no_args() -> Value {
    schema(json!({}), &[])
}

fn args<T: DeserializeOwned>(v: Value) -> anyhow::Result<T> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| ApiError::Validation(format!("аргументы: {e}")).into())
}

/// Инструмент с доступом к состоянию.
fn tool<F, Fut>(st: &McpState, name: &str, desc: &str, input: Value, f: F) -> McpTool
where
    F: Fn(McpState, Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<ToolOutput>> + Send + 'static,
{
    let st = st.clone();
    let f = Arc::new(f);
    McpTool::new(name, desc, input, move |a| {
        let st = st.clone();
        let f = f.clone();
        async move { f(st, a).await }
    })
}

fn svc(st: &McpState) -> &Arc<FocusService> {
    &st.service
}

/// Команда со статусом в ответе: разослать `status_changed` (как REST).
fn status_out(st: &McpState, prefix: &str, s: StatusDto) -> anyhow::Result<ToolOutput> {
    st.broadcast(ServerEvent::StatusChanged(s.clone()));
    let line = format!("{prefix} {}", fmt::status_line(&s));
    ToolOutput::new(line.trim(), &fmt::status_json(&s))
}

/// Завершение сессии: `session_ended` + `status_changed`.
fn end_out(st: &McpState, r: SessionEndResult) -> anyhow::Result<ToolOutput> {
    st.broadcast(ServerEvent::SessionEnded {
        session: r.ended.clone(),
    });
    st.broadcast(ServerEvent::StatusChanged(r.status.clone()));
    let line = format!(
        "{}. Сейчас: {}",
        capitalize(&fmt::session_line(&r.ended)),
        fmt::status_line(&r.status)
    );
    ToolOutput::new(
        line,
        &json!({"ended": fmt::session_json(&r.ended), "status": fmt::status_json(&r.status)}),
    )
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

async fn task_changed(st: &McpState, task_id: TaskId) {
    st.broadcast(ServerEvent::TaskChanged { task_id });
    if let Ok(s) = st.service.status().await {
        st.broadcast(ServerEvent::StatusChanged(s));
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    include_done: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddArgs {
    title: String,
    #[serde(default)]
    parent_id: Option<TaskId>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    planned_for: Option<NaiveDate>,
    #[serde(default)]
    estimate_intervals: Option<u32>,
}

#[derive(Deserialize)]
struct UpdateId {
    id: TaskId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkArgs {
    item_id: TaskId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FinishArgs {
    next: DoneNext,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DayArgs {
    #[serde(default)]
    date: Option<NaiveDate>,
}

/// Все инструменты ядра.
pub fn core_tools(st: &McpState) -> Vec<McpTool> {
    vec![
        tool(
            st,
            "get_status",
            "Текущее состояние: открыт ли день, активная сессия (работа/отдых/планирование), \
             ends_at и remaining_sec до её конца, отработано/норма дня в секундах. \
             events — что случилось с прошлого вызова get_status (сессия закончилась по лимиту, \
             ошибки плагинов). Вызывай в начале разговора и перед командами.",
            no_args(),
            |st, _| async move {
                let s = svc(&st).status().await?;
                let ev = st.feed.take_new();
                let mut data = fmt::status_json(&s);
                data["events"] = json!(fmt::events_json(&ev));
                if ev.missed > 0 {
                    data["events_missed"] = json!(ev.missed);
                }
                let mut line = fmt::status_line(&s);
                if let Some(e) = fmt::events_line(&ev) {
                    line = format!("{line} {e}");
                }
                ToolOutput::new(line, &data)
            },
        ),
        tool(
            st,
            "open_day",
            "Открыть сегодняшний рабочий день (или снова открыть закрытый). Обычно не нужно: \
             день открывается сам при старте любой сессии.",
            no_args(),
            |st, _| async move {
                let s = svc(&st).open_day().await?;
                status_out(&st, "День открыт.", s)
            },
        ),
        tool(
            st,
            "close_day",
            "Закрыть сегодняшний день (конец рабочего дня). Нельзя, пока идёт сессия.",
            no_args(),
            |st, _| async move {
                let s = svc(&st).close_day().await?;
                status_out(&st, "День закрыт.", s)
            },
        ),
        tool(
            st,
            "list_tasks",
            "Дерево задач: задачи верхнего уровня с пунктами (items). В работу берётся пункт — \
             узел без items и без done_items. По умолчанию только открытые (planned/in_progress); \
             done_items — сколько выполненных пунктов скрыто.",
            schema(
                json!({"include_done": {"type": "boolean", "description": "показать и выполненные/отменённые"}}),
                &[],
            ),
            |st, a| async move {
                let a: ListArgs = args(a)?;
                let tree = svc(&st).list_tasks().await?;
                let data: Vec<Value> = if a.include_done {
                    tree.iter().map(fmt::task_json).collect()
                } else {
                    fmt::open_tree_json(&tree)
                };
                let line = format!(
                    "Задач: {}, пунктов к работе: {}.",
                    data.len(),
                    fmt::workable_items(&tree)
                );
                ToolOutput::new(line, &data)
            },
        ),
        tool(
            st,
            "add_task",
            "Создать задачу или, с parent_id, пункт задачи. Глубина — два уровня: у пункта \
             пунктов нет. Пункт — маленький шаг, который можно сделать за одну-две сессии по 50 мин.",
            schema(
                json!({
                    "title": {"type": "string", "description": "название"},
                    "parent_id": {"type": "string", "description": "id задачи-родителя (для пункта)"},
                    "notes": {"type": "string", "description": "заметки"},
                    "planned_for": {"type": "string", "format": "date", "description": "на какой день, YYYY-MM-DD"},
                    "estimate_intervals": {"type": "integer", "minimum": 0, "description": "оценка в интервалах по 50 мин"},
                }),
                &["title"],
            ),
            |st, a| async move {
                let a: AddArgs = args(a)?;
                let t = svc(&st)
                    .create_task(NewTask {
                        parent_id: a.parent_id,
                        title: a.title,
                        notes: a.notes,
                        planned_for: a.planned_for,
                        estimate_intervals: a.estimate_intervals,
                    })
                    .await?;
                task_changed(&st, t.id).await;
                let what = if t.parent_id.is_some() {
                    "Пункт"
                } else {
                    "Задача"
                };
                ToolOutput::new(
                    format!("{what} «{}» создан(а), id {}.", t.title, t.id),
                    &fmt::task_json(&t),
                )
            },
        ),
        tool(
            st,
            "update_task",
            "Изменить задачу или пункт: название, заметки, статус (planned, in_progress, done, \
             cancelled), planned_for. Не переданные поля не меняются, null — очистить.",
            schema(
                json!({
                    "id": {"type": "string"},
                    "title": {"type": "string"},
                    "notes": {"type": ["string", "null"]},
                    "status": {"type": "string", "enum": ["planned", "in_progress", "done", "cancelled"]},
                    "planned_for": {"type": ["string", "null"], "format": "date"},
                    "estimate_intervals": {"type": ["integer", "null"], "minimum": 0},
                }),
                &["id"],
            ),
            |st, a| async move {
                let UpdateId { id } = args(a.clone())?;
                let mut patch_args = a;
                if let Some(o) = patch_args.as_object_mut() {
                    o.remove("id");
                }
                let patch: TaskPatch = args(patch_args)?;
                let t = svc(&st).update_task(id, patch).await?;
                task_changed(&st, t.id).await;
                ToolOutput::new(format!("Обновлено: «{}».", t.title), &fmt::task_json(&t))
            },
        ),
        tool(
            st,
            "start_work",
            &format!(
                "Начать сессию работы (50 мин, лимит в настройках) над одним пунктом. {NO_PAUSE} \
                 Сменить пункт посреди сессии нельзя. Закончить: finish_item (пункт готов) или \
                 interrupt (отвлёкся). По лимиту сервис сам закроет работу и начнёт отдых. \
                 Активное планирование закроется автоматически."
            ),
            schema(
                json!({"item_id": {"type": "string", "description": "id пункта из list_tasks"}}),
                &["item_id"],
            ),
            |st, a| async move {
                let a: WorkArgs = args(a)?;
                let s = svc(&st).start_work(a.item_id).await?;
                st.broadcast(ServerEvent::TaskChanged { task_id: a.item_id });
                status_out(&st, "Работа начата.", s)
            },
        ),
        tool(
            st,
            "finish_item",
            "«Готово»: пункт текущей работы выполнен — сессия закрывается с фактической \
             длительностью, пункт → done. next: rest — сразу отдых (10 мин, пропустить нельзя); \
             planning — сразу планирование (идёт в норму, после него — отдых).",
            schema(
                json!({"next": {"type": "string", "enum": ["rest", "planning"], "description": "что начать после"}}),
                &["next"],
            ),
            |st, a| async move {
                let a: FinishArgs = args(a)?;
                let r = svc(&st).done(a.next).await?;
                if let Some(task_id) = r.ended.task_id {
                    st.broadcast(ServerEvent::TaskChanged { task_id });
                }
                end_out(&st, r)
            },
        ),
        tool(
            st,
            "interrupt",
            &format!(
                "«Прервать» работу без завершения пункта (end_reason = interrupted). {NO_PAUSE} \
                 Это признание, что пользователь отвлёкся: эти 50 минут его ни для кого нет. \
                 Вызывай только по явной просьбе пользователя."
            ),
            no_args(),
            |st, _| async move {
                let r = svc(&st).interrupt().await?;
                end_out(&st, r)
            },
        ),
        tool(
            st,
            "start_planning",
            "Начать планирование: без лимита, отсчёт вверх, засчитывается в норму. \
             Заканчивается stop, стартом работы или отдыха. Нельзя во время работы или отдыха.",
            no_args(),
            |st, _| async move {
                let s = svc(&st).start_planning().await?;
                status_out(&st, "Планирование начато.", s)
            },
        ),
        tool(
            st,
            "stop",
            "«Стоп» планирования (только для планирования; работу заканчивают finish_item \
             или interrupt). После него начинается потерянное время — предложи следующий шаг.",
            no_args(),
            |st, _| async move {
                let r = svc(&st).stop().await?;
                end_out(&st, r)
            },
        ),
        tool(
            st,
            "start_rest",
            "Начать отдых (10 мин, лимит в настройках; пропустить нельзя). Нужен после \
             планирования; после finish_item(next=rest) отдых начинается сам.",
            no_args(),
            |st, _| async move {
                let s = svc(&st).start_rest().await?;
                status_out(&st, "Отдых начат.", s)
            },
        ),
        tool(
            st,
            "get_day_report",
            "Отчёт за день: сессии, итоги (работа, планирование, отдых, потерянное время в \
             секундах), норма. date — YYYY-MM-DD, по умолчанию сегодня.",
            schema(json!({"date": {"type": "string", "format": "date"}}), &[]),
            |st, a| async move {
                let a: DayArgs = args(a)?;
                let date = match a.date {
                    Some(d) => d,
                    None => {
                        let clock = &svc(&st).ports().clock;
                        clock.local_date(clock.now())
                    }
                };
                let d = svc(&st).day(date).await?;
                ToolOutput::new(fmt::day_line(&d), &fmt::day_json(&d))
            },
        ),
    ]
}

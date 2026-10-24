//! Компактные представления для модели: длительности по-русски, сводка
//! статуса, сокращённые задачи и сессии, подсказки к ошибкам.

use focus_api_types::{
    ActiveSession, ApiError, DayDto, EndReason, ServerEvent, SessionDto, SessionKind, StatusDto,
    TaskDto, TaskStatus,
};
use serde_json::{Map, Value, json};

use crate::feed::NewEvents;

/// `3725` → «1 ч 02 мин»; меньше минуты — «45 с».
pub fn dur(sec: i64) -> String {
    let sec = sec.max(0);
    if sec < 60 {
        return format!("{sec} с");
    }
    let min = sec / 60;
    let (h, m) = (min / 60, min % 60);
    if h == 0 {
        format!("{m} мин")
    } else {
        format!("{h} ч {m:02} мин")
    }
}

/// «5:20» — часы:минуты (для нормы дня).
pub fn hm(sec: i64) -> String {
    let min = sec.max(0) / 60;
    format!("{}:{:02}", min / 60, min % 60)
}

pub fn kind_ru(k: SessionKind) -> &'static str {
    match k {
        SessionKind::Work => "работа",
        SessionKind::Rest => "отдых",
        SessionKind::Planning => "планирование",
    }
}

pub fn reason_ru(r: EndReason) -> &'static str {
    match r {
        EndReason::TaskDone => "пункт готов",
        EndReason::Limit => "истёк лимит",
        EndReason::Interrupted => "прервана",
        EndReason::Stopped => "остановлена",
        EndReason::Switched => "переключение",
    }
}

fn snake<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// Секунды до конца активной сессии (у планирования — `None`).
pub fn remaining_sec(s: &StatusDto) -> Option<i64> {
    s.active
        .as_ref()
        .and_then(ActiveSession::ends_at)
        .map(|e| (e - s.server_time).num_seconds().max(0))
}

/// Компактный статус: `{now, day, active, ends_at, remaining_sec, worked_sec, target_sec, idle_since}`.
pub fn status_json(s: &StatusDto) -> Value {
    let mut o = Map::new();
    o.insert("now".into(), snake(s.server_time));
    o.insert(
        "day".into(),
        match &s.day {
            Some(d) => json!({
                "date": d.date,
                "opened_at": d.opened_at,
                "closed": d.closed_at.is_some(),
            }),
            None => Value::Null,
        },
    );
    let active = s.active.as_ref().map(|a| {
        let mut m = Map::new();
        m.insert("kind".into(), snake(a.kind()));
        m.insert("started_at".into(), snake(a.started_at()));
        m.insert(
            "elapsed_sec".into(),
            json!((s.server_time - a.started_at()).num_seconds().max(0)),
        );
        if let ActiveSession::Work { item, .. } = a {
            m.insert("item_id".into(), snake(item.id));
            m.insert("item".into(), json!(item.title));
            if let Some(p) = &item.parent_title {
                m.insert("parent".into(), json!(p));
            }
        }
        Value::Object(m)
    });
    o.insert("active".into(), active.unwrap_or(Value::Null));
    o.insert(
        "ends_at".into(),
        snake(s.active.as_ref().and_then(ActiveSession::ends_at)),
    );
    o.insert("remaining_sec".into(), snake(remaining_sec(s)));
    o.insert("worked_sec".into(), json!(s.worked_today_sec));
    o.insert("target_sec".into(), json!(s.target_sec));
    o.insert("idle_since".into(), snake(s.idle_since));
    Value::Object(o)
}

/// Одна строка: что идёт сейчас и сколько набрано.
pub fn status_line(s: &StatusDto) -> String {
    let norm = format!("норма {} из {}", hm(s.worked_today_sec), hm(s.target_sec));
    let now = match &s.active {
        Some(ActiveSession::Work { item, .. }) => format!(
            "Работа над «{}», осталось {}",
            item.title,
            dur(remaining_sec(s).unwrap_or(0))
        ),
        Some(ActiveSession::Rest { .. }) => {
            format!("Отдых, осталось {}", dur(remaining_sec(s).unwrap_or(0)))
        }
        Some(ActiveSession::Planning { started_at, .. }) => {
            format!(
                "Планирование, идёт {}",
                dur((s.server_time - *started_at).num_seconds())
            )
        }
        None => match (&s.day, s.idle_since) {
            (None, _) => "День не открыт".to_owned(),
            (Some(d), _) if d.closed_at.is_some() => "День закрыт".to_owned(),
            (Some(_), Some(since)) => format!(
                "Нет сессии — потерянное время уже {}",
                dur((s.server_time - since).num_seconds())
            ),
            (Some(_), None) => "Нет сессии".to_owned(),
        },
    };
    format!("{now}; {norm}.")
}

/// Сессия кратко.
pub fn session_json(s: &SessionDto) -> Value {
    let mut m = Map::new();
    m.insert("kind".into(), snake(s.kind));
    m.insert("started_at".into(), snake(s.started_at));
    m.insert("ended_at".into(), snake(s.ended_at));
    m.insert("duration_sec".into(), json!(s.duration_sec));
    if let Some(r) = s.end_reason {
        m.insert("end_reason".into(), snake(r));
    }
    if let Some(t) = &s.task_title {
        m.insert("item".into(), json!(t));
    }
    Value::Object(m)
}

/// «работа над «x» закрыта: пункт готов, 32 мин».
pub fn session_line(s: &SessionDto) -> String {
    let what = match &s.task_title {
        Some(t) => format!("{} над «{t}»", kind_ru(s.kind)),
        None => kind_ru(s.kind).to_owned(),
    };
    let why = s.end_reason.map_or("", reason_ru);
    format!("{what} закрыта ({why}, {})", dur(s.duration_sec))
}

/// События ленты для `get_status`.
pub fn events_json(n: &NewEvents) -> Vec<Value> {
    n.items
        .iter()
        .filter_map(|i| {
            let mut v = match &i.event {
                ServerEvent::SessionEnded { session } => {
                    let mut v = session_json(session);
                    v["type"] = json!("session_ended");
                    v
                }
                ServerEvent::PluginError { plugin, message } => {
                    json!({"type": "plugin_error", "plugin": plugin, "message": message})
                }
                _ => return None,
            };
            v["seq"] = json!(i.seq);
            Some(v)
        })
        .collect()
}

/// Строка о событиях: «С прошлого раза: …».
pub fn events_line(n: &NewEvents) -> Option<String> {
    let mut parts: Vec<String> = n
        .items
        .iter()
        .filter_map(|i| match &i.event {
            ServerEvent::SessionEnded { session } => Some(session_line(session)),
            ServerEvent::PluginError { plugin, message } => {
                Some(format!("ошибка плагина {plugin}: {message}"))
            }
            _ => None,
        })
        .collect();
    if n.missed > 0 {
        parts.push(format!("ещё {} событий потеряно", n.missed));
    }
    (!parts.is_empty()).then(|| format!("С прошлого раза: {}.", parts.join("; ")))
}

/// Задача кратко: `{id, title, status, notes?, planned_for?, url?, items?}`.
pub fn task_json(t: &TaskDto) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), snake(t.id));
    m.insert("title".into(), json!(t.title));
    m.insert("status".into(), snake(t.status));
    if let Some(n) = &t.notes {
        m.insert("notes".into(), json!(n));
    }
    if let Some(d) = t.planned_for {
        m.insert("planned_for".into(), snake(d));
    }
    if let Some(e) = t.estimate_intervals {
        m.insert("estimate_intervals".into(), json!(e));
    }
    if let Some(u) = &t.external_url {
        m.insert("url".into(), json!(u));
    }
    if !t.children.is_empty() {
        m.insert(
            "items".into(),
            Value::Array(t.children.iter().map(task_json).collect()),
        );
    }
    Value::Object(m)
}

fn closed(s: TaskStatus) -> bool {
    matches!(s, TaskStatus::Done | TaskStatus::Cancelled)
}

/// Только открытые узлы: закрытые пункты скрыты, у задачи остаётся
/// `done_items` — сколько пунктов скрыто (чтобы задача без видимых пунктов
/// не выглядела пунктом для работы). Закрытая задача без открытых пунктов
/// скрыта целиком.
pub fn open_tree_json(tree: &[TaskDto]) -> Vec<Value> {
    tree.iter()
        .filter_map(|t| {
            let open: Vec<&TaskDto> = t.children.iter().filter(|c| !closed(c.status)).collect();
            if closed(t.status) && open.is_empty() {
                return None;
            }
            let hidden = t.children.len() - open.len();
            let mut v = task_json(&TaskDto {
                children: Vec::new(),
                ..t.clone()
            });
            if !open.is_empty() {
                v["items"] = Value::Array(open.into_iter().map(task_json).collect());
            }
            if hidden > 0 {
                v["done_items"] = json!(hidden);
            }
            Some(v)
        })
        .collect()
}

/// Сколько пунктов можно взять в работу (открытые узлы без детей).
pub fn workable_items(tree: &[TaskDto]) -> usize {
    tree.iter()
        .map(|t| {
            if t.children.is_empty() {
                usize::from(!closed(t.status))
            } else {
                t.children.iter().filter(|c| !closed(c.status)).count()
            }
        })
        .sum()
}

/// Отчёт за день.
pub fn day_json(d: &DayDto) -> Value {
    json!({
        "date": d.date,
        "opened_at": d.day.as_ref().map(|x| x.opened_at),
        "closed_at": d.day.as_ref().and_then(|x| x.closed_at),
        "totals": d.totals,
        "sessions": d.sessions.iter().map(session_json).collect::<Vec<_>>(),
        "lost": d.lost,
    })
}

pub fn day_line(d: &DayDto) -> String {
    if d.day.is_none() {
        return format!("{}: день не открывался.", d.date);
    }
    let t = &d.totals;
    format!(
        "{}: норма {} из {} (работа {}, планирование {}); отдых {}, потеряно {}; сессий: {}.",
        d.date,
        hm(t.worked_sec),
        hm(t.target_sec),
        dur(t.work_sec),
        dur(t.planning_sec),
        dur(t.rest_sec),
        dur(t.lost_sec),
        d.sessions.len()
    )
}

/// Подсказка модели, что делать после ошибки.
pub fn error_hint(e: &ApiError) -> Option<&'static str> {
    Some(match e {
        ApiError::NoActiveSession => "Сейчас нет сессии — проверь get_status",
        ApiError::SessionAlreadyActive => {
            "Уже идёт работа или отдых; работу заканчивают finish_item или interrupt, отдых пропустить нельзя"
        }
        ApiError::WrongSessionKind { .. } => {
            "Команда не к той сессии: finish_item/interrupt — для работы, stop — для планирования"
        }
        ApiError::ItemRequired => {
            "В работу берётся пункт (узел без детей) — выбери пункт задачи через list_tasks"
        }
        ApiError::ItemNotFound => "Нет такого id — возьми id из list_tasks",
        ApiError::DayClosed => "Сегодняшний день закрыт — открой его open_day",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(dur(45), "45 с");
        assert_eq!(dur(60), "1 мин");
        assert_eq!(dur(3725), "1 ч 02 мин");
        assert_eq!(dur(-5), "0 с");
        assert_eq!(hm(5 * 3600 + 20 * 60), "5:20");
    }
}

use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use uuid::Uuid;

use super::*;

fn t(min: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 24, 9, min, 0).unwrap()
}

fn id<T: From<Uuid>>(n: u128) -> T {
    T::from(Uuid::from_u128(n))
}

fn round_trip<T>(value: &T) -> serde_json::Value
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_value(value).unwrap();
    let back: T = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(&back, value);
    json
}

fn status() -> StatusDto {
    StatusDto {
        server_time: t(10),
        day: None,
        active: Some(ActiveSession::Planning {
            session_id: id(3),
            started_at: t(5),
        }),
        worked_today_sec: 300,
        target_sec: 8 * 3600,
        idle_since: None,
    }
}

fn session() -> SessionDto {
    SessionDto {
        id: id(1),
        day_id: id(2),
        kind: SessionKind::Work,
        task_id: Some(id(4)),
        task_title: Some("пункт".into()),
        started_at: t(0),
        ended_at: Some(t(50)),
        limit_sec: Some(3000),
        end_reason: Some(EndReason::Limit),
        duration_sec: 3000,
    }
}

#[test]
fn active_session_round_trip() {
    let work = ActiveSession::Work {
        session_id: id(1),
        item: ItemRef {
            id: id(2),
            title: "Написать тесты".into(),
            parent_id: None,
            parent_title: None,
        },
        started_at: t(0),
        ends_at: t(50),
    };
    let json = round_trip(&work);
    assert_eq!(json["kind"], "work");
    assert_eq!(json["ends_at"], "2026-10-24T09:50:00Z");
    assert_eq!(work.ends_at(), Some(t(50)));
    assert_eq!(work.kind(), SessionKind::Work);

    let rest = ActiveSession::Rest {
        session_id: id(1),
        started_at: t(50),
        ends_at: t(59),
    };
    assert_eq!(round_trip(&rest)["kind"], "rest");

    let planning = ActiveSession::Planning {
        session_id: id(1),
        started_at: t(0),
    };
    let json = round_trip(&planning);
    assert_eq!(json["kind"], "planning");
    assert!(json.get("ends_at").is_none());
    assert_eq!(planning.ends_at(), None);
}

#[test]
fn active_work_without_item_is_rejected() {
    let json = json!({
        "kind": "work",
        "session_id": Uuid::from_u128(1),
        "started_at": "2026-10-24T09:00:00Z",
        "ends_at": "2026-10-24T09:50:00Z",
    });
    assert!(serde_json::from_value::<ActiveSession>(json).is_err());
}

#[test]
fn api_error_round_trip() {
    let all = [
        ApiError::NoActiveSession,
        ApiError::SessionAlreadyActive,
        ApiError::WrongSessionKind {
            expected: SessionKind::Planning,
            actual: SessionKind::Work,
        },
        ApiError::ItemRequired,
        ApiError::ItemNotFound,
        ApiError::DayClosed,
        ApiError::PluginUnavailable("jira".into()),
        ApiError::Validation("пустое название".into()),
        ApiError::Unauthorized,
        ApiError::Forbidden("host".into()),
        ApiError::Transport("timeout".into()),
        ApiError::Internal("db".into()),
        ApiError::Unknown,
    ];
    for err in &all {
        let json = round_trip(err);
        assert_eq!(json["code"], err.code(), "{err:?}");
        assert!((400..600).contains(&err.http_status()));
    }
    assert_eq!(
        serde_json::to_value(ApiError::NoActiveSession).unwrap(),
        json!({"code": "no_active_session"})
    );
    assert_eq!(
        serde_json::to_value(ApiError::Validation("x".into())).unwrap(),
        json!({"code": "validation", "details": "x"})
    );
    assert_eq!(ApiError::ItemNotFound.http_status(), 404);
    assert_eq!(ApiError::SessionAlreadyActive.http_status(), 409);
}

#[test]
fn api_error_unknown_code() {
    let err: ApiError = serde_json::from_value(json!({"code": "quota_exceeded"})).unwrap();
    assert_eq!(err, ApiError::Unknown);
}

#[test]
fn server_event_round_trip() {
    let events = [
        ServerEvent::StatusChanged(status()),
        ServerEvent::SessionEnded { session: session() },
        ServerEvent::TaskChanged { task_id: id(7) },
        ServerEvent::PluginError {
            plugin: "jira".into(),
            message: "401".into(),
        },
    ];
    for ev in &events {
        let json = round_trip(ev);
        assert_eq!(json["type"], ev.name());
    }
}

#[test]
fn server_event_unknown_type() {
    let ev: ServerEvent = serde_json::from_value(json!({"type": "achievement_unlocked"})).unwrap();
    assert_eq!(ev, ServerEvent::Unknown);
    let ev: ServerEvent = serde_json::from_str(r#"{"type":"something_new"}"#).unwrap();
    assert_eq!(ev, ServerEvent::Unknown);
}

#[test]
fn dto_tolerates_missing_optional_fields() {
    let status: StatusDto = serde_json::from_value(json!({
        "server_time": "2026-10-24T09:00:00Z",
        "worked_today_sec": 0,
        "target_sec": 28800,
    }))
    .unwrap();
    assert_eq!(status.active, None);
    assert_eq!(status.day, None);

    let settings: SettingsDto = serde_json::from_value(json!({
        "work_min": 50, "rest_min": 10, "daily_target_min": 480,
    }))
    .unwrap();
    assert_eq!(settings, SettingsDto::default());
}

#[test]
fn dto_ignores_unknown_fields() {
    let status: StatusDto = serde_json::from_value(json!({
        "server_time": "2026-10-24T09:00:00Z",
        "worked_today_sec": 0,
        "target_sec": 28800,
        "streak_days": 5,
    }))
    .unwrap();
    assert_eq!(status.worked_today_sec, 0);
}

#[test]
fn task_patch_distinguishes_absent_and_null() {
    let patch: TaskPatch = serde_json::from_value(json!({"title": "x"})).unwrap();
    assert_eq!(patch.notes, None);

    let patch: TaskPatch = serde_json::from_value(json!({"notes": null})).unwrap();
    assert_eq!(patch.notes, Some(None));

    let patch = TaskPatch {
        planned_for: Some(None),
        status: Some(TaskStatus::Done),
        ..TaskPatch::default()
    };
    let json = round_trip(&patch);
    assert_eq!(json, json!({"planned_for": null, "status": "done"}));
}

#[test]
fn task_tree_helpers() {
    let leaf = TaskDto {
        id: id(2),
        parent_id: Some(id(1)),
        title: "пункт".into(),
        notes: None,
        status: TaskStatus::Planned,
        planned_for: None,
        estimate_intervals: None,
        sort_order: 0,
        created_at: t(0),
        done_at: None,
        external_url: None,
        children: vec![],
    };
    let root = TaskDto {
        id: id(1),
        parent_id: None,
        title: "задача".into(),
        children: vec![leaf.clone()],
        ..leaf.clone()
    };
    assert!(!root.is_item());
    assert!(leaf.is_item());
    assert_eq!(root.find(id(2)), Some(&leaf));
    round_trip(&root);
}

#[test]
fn ids_are_plain_uuid_strings() {
    let task: TaskId = id(42);
    let json = serde_json::to_value(task).unwrap();
    assert_eq!(json, json!(Uuid::from_u128(42).to_string()));
    assert_eq!(task.to_string().parse::<TaskId>().unwrap(), task);
}

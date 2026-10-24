-- Таблицы ядра Focus Flow (SPEC «Данные (ядро)»).
-- UUID — TEXT (нижний регистр, с дефисами). Время — TEXT RFC3339 UTC
-- фиксированной длины с наносекундами (`2026-10-24T09:00:00.000000000Z`):
-- строки сравниваются так же, как моменты времени. Даты — TEXT `YYYY-MM-DD`.

CREATE TABLE work_days (
    id          TEXT PRIMARY KEY NOT NULL,
    date        TEXT NOT NULL UNIQUE,
    opened_at   TEXT NOT NULL,
    closed_at   TEXT,
    -- норма дня в секундах (SPEC: target_minutes; в домене — секунды)
    target_sec  INTEGER NOT NULL CHECK (target_sec >= 0)
) STRICT;

CREATE TABLE tasks (
    id                  TEXT PRIMARY KEY NOT NULL,
    parent_id           TEXT REFERENCES tasks(id),
    title               TEXT NOT NULL,
    notes               TEXT,
    status              TEXT NOT NULL
        CHECK (status IN ('planned', 'in_progress', 'done', 'cancelled')),
    planned_for         TEXT,
    estimate_intervals  INTEGER CHECK (estimate_intervals >= 0),
    sort_order          INTEGER NOT NULL,
    created_at          TEXT NOT NULL,
    done_at             TEXT,
    external_url        TEXT
) STRICT;

CREATE INDEX tasks_parent ON tasks(parent_id, sort_order, created_at, id);
CREATE INDEX tasks_external_url ON tasks(external_url) WHERE external_url IS NOT NULL;

CREATE TABLE sessions (
    id          TEXT PRIMARY KEY NOT NULL,
    day_id      TEXT NOT NULL REFERENCES work_days(id),
    kind        TEXT NOT NULL CHECK (kind IN ('work', 'rest', 'planning')),
    task_id     TEXT REFERENCES tasks(id),
    started_at  TEXT NOT NULL,
    ended_at    TEXT,
    limit_sec   INTEGER,
    end_reason  TEXT
        CHECK (end_reason IN ('task_done', 'limit', 'interrupted', 'stopped', 'switched')),
    CHECK ((kind = 'work') = (task_id IS NOT NULL)),
    CHECK ((ended_at IS NULL) = (end_reason IS NULL))
) STRICT;

CREATE INDEX sessions_day ON sessions(day_id, started_at);
-- Активная сессия — максимум одна.
CREATE UNIQUE INDEX sessions_one_active ON sessions(coalesce(ended_at, '')) WHERE ended_at IS NULL;

-- Настройки: ключ → JSON-значение (work_min, rest_min, daily_target_min, plugins).
CREATE TABLE settings (
    key    TEXT PRIMARY KEY NOT NULL,
    value  TEXT NOT NULL
) STRICT;

-- Outbox: одно событие для одного плагина. id растёт в порядке публикации.
CREATE TABLE outbox (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    event_json       TEXT NOT NULL,
    plugin_id        TEXT NOT NULL,
    attempts         INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at  TEXT,
    last_error       TEXT,
    last_error_at    TEXT,
    done_at          TEXT
) STRICT;

CREATE INDEX outbox_pending ON outbox(plugin_id, id) WHERE done_at IS NULL;

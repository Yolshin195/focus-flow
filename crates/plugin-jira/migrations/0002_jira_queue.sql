-- Очередь отправки в Jira: что плагин собирается записать. События
-- outbox раскладываются сюда без сети; отправка — кнопкой «Отправить» или
-- сразу, если включено `auto_send`.

CREATE TABLE jira_queue (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    -- checklist_add — строка `- текст`; checklist_done — `+ текст`; worklog.
    kind           TEXT NOT NULL CHECK (kind IN ('checklist_add', 'checklist_done', 'worklog')),
    -- Пункт (checklist_*) или сессия (worklog).
    ref_id         TEXT NOT NULL,
    issue_key      TEXT NOT NULL,
    -- Текст пункта или комментарий worklog.
    text           TEXT NOT NULL,
    -- Только worklog: начало (RFC 3339) и длительность.
    started_at     TEXT,
    seconds        INTEGER,
    created_at     TEXT NOT NULL,
    -- pending — ждёт; sent — отправлено; dropped — удалено пользователем
    -- (строка остаётся, чтобы повтор того же события не вернул запись).
    status         TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sent', 'dropped')),
    attempts       INTEGER NOT NULL DEFAULT 0,
    last_error     TEXT,
    last_error_at  TEXT,
    UNIQUE (kind, ref_id)
) STRICT;

CREATE INDEX jira_queue_pending ON jira_queue(status, id);

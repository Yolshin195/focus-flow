-- Синхронизация из Jira и снятие отметки.

-- Последнее известное состояние строки в Jira (planned / done / in_progress /
-- cancelled): база, по которой синхронизация отличает изменение в Jira от
-- изменения у нас. NULL — ещё не знаем (записи до этой миграции).
ALTER TABLE jira_items ADD COLUMN jira_status TEXT;

-- Название задачи, которое мы последний раз взяли из Jira («KEY · summary»):
-- переименованную у нас задачу синхронизация не трогает.
ALTER TABLE jira_links ADD COLUMN title TEXT;

-- Очередь: новый вид checklist_undone (`+` → `-`). Отметка и снятие одного
-- пункта могут повторяться, поэтому уникальность (вид, пункт) осталась только
-- у checklist_add и worklog — частичным индексом. SQLite не меняет CHECK у
-- таблицы, поэтому она пересоздаётся.
CREATE TABLE jira_queue_new (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    kind           TEXT NOT NULL CHECK (kind IN ('checklist_add', 'checklist_done', 'checklist_undone', 'worklog')),
    ref_id         TEXT NOT NULL,
    issue_key      TEXT NOT NULL,
    text           TEXT NOT NULL,
    started_at     TEXT,
    seconds        INTEGER,
    created_at     TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sent', 'dropped')),
    attempts       INTEGER NOT NULL DEFAULT 0,
    last_error     TEXT,
    last_error_at  TEXT
) STRICT;

INSERT INTO jira_queue_new (id, kind, ref_id, issue_key, text, started_at, seconds, created_at,
                            status, attempts, last_error, last_error_at)
SELECT id, kind, ref_id, issue_key, text, started_at, seconds, created_at,
       status, attempts, last_error, last_error_at
FROM jira_queue;

DROP TABLE jira_queue;
ALTER TABLE jira_queue_new RENAME TO jira_queue;

CREATE INDEX jira_queue_pending ON jira_queue(status, id);
CREATE INDEX jira_queue_ref ON jira_queue(ref_id, status);
CREATE UNIQUE INDEX jira_queue_once ON jira_queue(kind, ref_id)
    WHERE kind IN ('checklist_add', 'worklog');

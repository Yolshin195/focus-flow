-- Таблицы плагина Jira (SPEC «Плагин Jira»). Журнал миграций —
-- `_sqlx_migrations_jira`, отдельно от ядра.

-- Наша задача верхнего уровня ↔ задача Jira.
CREATE TABLE jira_links (
    task_id    TEXT PRIMARY KEY NOT NULL,
    issue_key  TEXT NOT NULL,
    url        TEXT NOT NULL
) STRICT;

CREATE INDEX jira_links_issue ON jira_links(issue_key);

-- Наш пункт ↔ строка Smart Checklist (у строк нет id — сопоставление по тексту).
CREATE TABLE jira_items (
    item_id    TEXT PRIMARY KEY NOT NULL,
    issue_key  TEXT NOT NULL,
    line_text  TEXT NOT NULL
) STRICT;

-- Отправленные worklog: повторная доставка того же события не дублирует запись.
CREATE TABLE jira_worklogs (
    session_id  TEXT PRIMARY KEY NOT NULL,
    issue_key   TEXT NOT NULL,
    worklog_id  TEXT,
    status      TEXT NOT NULL CHECK (status IN ('sent', 'skipped'))
) STRICT;

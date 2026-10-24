//! Экран «Настройки»: подключение к Jira, состояние плагина и очередь
//! отправки (что плагин собирается записать в Jira).
//!
//! Данные — через [`JiraSettingsApi`](crate::api::JiraSettingsApi) из
//! [`ApiHandle`](crate::ApiHandle) и `FocusApi::plugin_status`. Форма
//! заполняется из сохранённых настроек; токен только на запись — поле пустое,
//! рядом пометка «сохранён».

use std::time::Duration;

use dioxus::prelude::*;
use focus_api_types::{ApiError, PluginStatusDto};

use crate::api::{JiraMode, JiraOp, JiraOpKind, JiraPatch, JiraQueue, JiraState, JiraWorklog};
use crate::components::{Icon, TextInput};
use crate::fmt;
use crate::state::{error_text, use_api};

/// Как часто обновлять состояние плагина (очередь, последняя ошибка).
const STATUS_POLL: Duration = Duration::from_secs(10);

/// Значения формы.
#[derive(Debug, Clone, PartialEq, Default)]
struct Form {
    base_url: String,
    mode: JiraMode,
    email: String,
    token: String,
    field: String,
    worklog: JiraWorklog,
    auto_send: bool,
}

impl Form {
    fn from_state(s: &JiraState) -> Self {
        Self {
            base_url: s.base_url.clone().unwrap_or_default(),
            mode: s.mode,
            email: s.email.clone().unwrap_or_default(),
            token: String::new(),
            field: s.checklist_field_id.clone().unwrap_or_default(),
            worklog: s.worklog,
            auto_send: s.auto_send,
        }
    }

    fn patch(&self) -> JiraPatch {
        let token = self.token.trim();
        // Адрес без схемы — почти всегда https: подставим её сами.
        let base = self.base_url.trim();
        let base_url = if base.is_empty() || base.contains("://") {
            base.to_owned()
        } else {
            format!("https://{base}")
        };
        JiraPatch {
            base_url,
            mode: self.mode,
            // Email нужен только Cloud; в Data Center поле скрыто и не трогается.
            email: self.email.trim().to_owned(),
            checklist_field_id: self.field.trim().to_owned(),
            worklog: self.worklog,
            auto_send: self.auto_send,
            token: (!token.is_empty()).then(|| token.to_owned()),
        }
    }
}

/// Результат последнего действия под кнопками.
#[derive(Debug, Clone, PartialEq)]
enum Notice {
    Ok(String),
    Err(String),
}

/// Состояние экрана.
#[derive(Clone, Copy, PartialEq)]
struct Page {
    saved: Signal<Option<JiraState>>,
    form: Signal<Form>,
    /// Растёт, когда форму заполнили заново, — поля пересоздаются.
    form_rev: Signal<u32>,
    plugin: Signal<Option<PluginStatusDto>>,
    load_error: Signal<Option<String>>,
    notice: Signal<Option<Notice>>,
    busy: Signal<bool>,
    queue: Signal<Option<JiraQueue>>,
    queue_notice: Signal<Option<Notice>>,
    queue_busy: Signal<bool>,
}

/// Перечитать настройки и состояние плагина. `reset_form` — заполнить
/// форму сохранёнными значениями (при открытии и после сохранения).
async fn reload(api: &crate::ApiHandle, mut p: Page, reset_form: bool) {
    match api.jira().jira_settings().await {
        Ok(s) => {
            if reset_form || p.saved.peek().is_none() {
                p.form.set(Form::from_state(&s));
                p.form_rev += 1;
            }
            if p.saved.peek().as_ref() != Some(&s) {
                p.saved.set(Some(s));
            }
            if p.load_error.peek().is_some() {
                p.load_error.set(None);
            }
        }
        Err(e) => p.load_error.set(Some(load_error_text(&e))),
    }
    if let Ok(q) = api.jira().jira_queue().await {
        set_queue(p, q);
    }
    if let Ok(list) = api.0.plugin_status().await {
        let jira = list.into_iter().find(|s| s.id == "jira");
        if *p.plugin.peek() != jira {
            p.plugin.set(jira);
        }
    }
}

/// Положить очередь, если она изменилась (итог отправки — отдельно, в notice).
fn set_queue(mut p: Page, q: JiraQueue) {
    let q = JiraQueue {
        sent: None,
        error: None,
        ..q
    };
    if p.queue.peek().as_ref() != Some(&q) {
        p.queue.set(Some(q));
    }
}

fn load_error_text(e: &ApiError) -> String {
    match e {
        ApiError::Transport(m) if m.contains("404") => {
            "Плагин Jira не собран в этот сервис (cargo feature «jira»).".into()
        }
        other => error_text(other),
    }
}

/// Экран «Настройки».
#[component]
pub fn Settings() -> Element {
    let api = use_api();
    let page = Page {
        saved: use_signal(|| None),
        form: use_signal(Form::default),
        form_rev: use_signal(|| 0),
        plugin: use_signal(|| None),
        load_error: use_signal(|| None),
        notice: use_signal(|| None),
        busy: use_signal(|| false),
        queue: use_signal(|| None),
        queue_notice: use_signal(|| None),
        queue_busy: use_signal(|| false),
    };

    let api_poll = api.clone();
    use_future(move || {
        let api = api_poll.clone();
        async move {
            reload(&api, page, true).await;
            loop {
                futures_timer::Delay::new(STATUS_POLL).await;
                reload(&api, page, false).await;
            }
        }
    });

    let mut form = page.form;
    let notice = page.notice;
    let saved = page.saved.read().clone();
    let f = form.read().clone();
    let dirty = is_dirty(saved.as_ref(), &f);
    let busy = (page.busy)();
    let rev = (page.form_rev)();
    let cloud = f.mode == JiraMode::Cloud;
    let token_hint: &'static str = if cloud {
        "API-токен: id.atlassian.com → Security → API tokens"
    } else {
        "Персональный токен (PAT): профиль → Personal Access Tokens"
    };
    let has_token = saved.as_ref().is_some_and(|s| s.has_token);
    let token_placeholder = if has_token {
        "сохранён — введи новый, чтобы заменить"
    } else {
        "вставь токен"
    }
    .to_owned();

    let api_save = api.clone();
    let save = move |then_test: bool| {
        let api = api_save.clone();
        let mut p = page;
        spawn(async move {
            p.busy.set(true);
            p.notice.set(None);
            let patch = p.form.peek().patch();
            let dirty = is_dirty(p.saved.peek().as_ref(), &p.form.peek());
            let saved_ok = if dirty || p.saved.peek().is_none() {
                match api.jira().save_jira_settings(patch).await {
                    Ok(_) => {
                        reload(&api, p, true).await;
                        if !then_test {
                            p.notice.set(Some(Notice::Ok("Сохранено.".into())));
                        }
                        true
                    }
                    Err(e) => {
                        p.notice.set(Some(Notice::Err(save_error_text(&e))));
                        false
                    }
                }
            } else {
                true
            };
            if then_test && saved_ok {
                let result = api.jira().test_jira().await;
                p.notice.set(Some(match result {
                    Ok(r) => Notice::Ok(format!("Подключено: {} · {}", r.user, r.base_url)),
                    Err(e) => Notice::Err(test_error_text(&e)),
                }));
                reload(&api, p, false).await;
            }
            p.busy.set(false);
        });
    };
    let save_click = save.clone();
    let test_click = save.clone();
    let save_enter = save;

    let api_forget = api.clone();
    let forget_token = move |_| {
        let api = api_forget.clone();
        let mut p = page;
        spawn(async move {
            p.busy.set(true);
            let patch = JiraPatch {
                token: Some(String::new()),
                ..Form::from_state(p.saved.peek().as_ref().unwrap_or(&JiraState::default())).patch()
            };
            match api.jira().save_jira_settings(patch).await {
                Ok(_) => p.notice.set(Some(Notice::Ok("Токен удалён.".into()))),
                Err(e) => p.notice.set(Some(Notice::Err(save_error_text(&e)))),
            }
            reload(&api, p, false).await;
            p.busy.set(false);
        });
    };

    // `Callback` — `Copy`: один обработчик на все поля формы.
    let on_enter = use_callback(move |e: KeyboardEvent| {
        if e.key() == Key::Enter && !*page.busy.peek() {
            save_enter(false);
        }
    });

    rsx! {
        div { class: "page settings",
            header { class: "page-head",
                div {
                    h1 { class: "page-title", "Настройки" }
                    p { class: "page-sub", "Подключение к Jira: импорт задач, чек-лист и worklog" }
                }
            }

            if let Some(err) = page.load_error.read().clone() {
                div { class: "settings-alert", role: "alert", "{err}" }
            }

            section { class: "card settings-card", "aria-labelledby": "jira-title",
                div { class: "card-head",
                    div { class: "card-head-title",
                        span { class: "chip chip-jira", "JIRA" }
                        h2 { id: "jira-title", class: "card-title", "Jira" }
                    }
                    PluginBadge { state: saved.clone(), plugin: page.plugin.read().clone() }
                }

                div { class: "form",
                    FormRow { label: "Адрес Jira", hint: "Например, https://jira.example.com или https://example.atlassian.net",
                        label { class: "field",
                            TextInput {
                                reset: rev,
                                initial: f.base_url.clone(),
                                kind: "url",
                                placeholder: "https://jira.example.com",
                                label: "Адрес Jira",
                                spellcheck: false,
                                on_input: move |v| form.write().base_url = v,
                                on_keydown: move |e| on_enter.call(e),
                            }
                        }
                    }
                    FormRow { label: "Режим", hint: "",
                        div { class: "seg", role: "radiogroup", "aria-label": "Режим Jira",
                            for (mode , title) in [(JiraMode::DataCenter, "Data Center / Server"), (JiraMode::Cloud, "Cloud")] {
                                button {
                                    class: if f.mode == mode { "seg-btn active" } else { "seg-btn" },
                                    role: "radio",
                                    "aria-checked": "{f.mode == mode}",
                                    onclick: move |_| form.write().mode = mode,
                                    "{title}"
                                }
                            }
                        }
                    }
                    if cloud {
                        FormRow { label: "Email", hint: "Email учётной записи Atlassian",
                            label { class: "field",
                                TextInput {
                                    reset: rev,
                                    initial: f.email.clone(),
                                    kind: "email",
                                    placeholder: "me@example.com",
                                    label: "Email",
                                    spellcheck: false,
                                    on_input: move |v| form.write().email = v,
                                    on_keydown: move |e| on_enter.call(e),
                                }
                            }
                        }
                    }
                    FormRow {
                        label: "Токен",
                        hint: token_hint,
                        div { class: "field-line",
                            label { class: "field field-grow",
                                TextInput {
                                    reset: rev,
                                    kind: "password",
                                    placeholder: token_placeholder,
                                    label: "Токен",
                                    spellcheck: false,
                                    on_input: move |v| form.write().token = v,
                                    on_keydown: move |e| on_enter.call(e),
                                }
                                if has_token && f.token.is_empty() {
                                    span { class: "saved-mark", {Icon::Check.render(14)} "сохранён" }
                                }
                            }
                            if has_token {
                                button {
                                    class: "btn btn-ghost btn-small",
                                    disabled: busy,
                                    onclick: forget_token,
                                    "Удалить"
                                }
                            }
                        }
                    }
                    FormRow {
                        label: "Поле чек-листа",
                        hint: "Необязательно: id текстового поля «Checklists» из Smart Checklist. Пусто — плагин найдёт его сам по имени.",
                        label { class: "field",
                            TextInput {
                                reset: rev,
                                initial: f.field.clone(),
                                placeholder: "автоматически",
                                label: "ID поля чек-листа",
                                spellcheck: false,
                                on_input: move |v| form.write().field = v,
                                on_keydown: move |e| on_enter.call(e),
                            }
                            if let Some(id) = saved.as_ref().and_then(|s| s.checklist_field_resolved.clone()).filter(|_| f.field.trim().is_empty()) {
                                span { class: "resolved num", title: "Найдено автоматически", "{id}" }
                            }
                        }
                    }
                    FormRow {
                        label: "Отправка",
                        hint: "Выключено — изменения копятся в очереди ниже и уходят в Jira кнопкой «Отправить»",
                        div { class: "toggles",
                            Toggle {
                                label: "Отправлять автоматически",
                                on: f.auto_send,
                                on_toggle: move |v| form.write().auto_send = v,
                            }
                        }
                    }
                    FormRow { label: "Worklog", hint: "Какие сессии списывать на задачу Jira",
                        div { class: "toggles",
                            Toggle {
                                label: "Работа",
                                on: f.worklog.work,
                                on_toggle: move |v| form.write().worklog.work = v,
                            }
                            Toggle {
                                label: "Прерванная работа",
                                on: f.worklog.interrupted,
                                on_toggle: move |v| form.write().worklog.interrupted = v,
                            }
                            Toggle {
                                label: "Планирование",
                                on: f.worklog.planning,
                                on_toggle: move |v| form.write().worklog.planning = v,
                            }
                        }
                    }
                }

                div { class: "card-foot",
                    button {
                        class: "btn btn-accent",
                        disabled: busy || !dirty,
                        onclick: move |_| save_click(false),
                        "Сохранить"
                    }
                    button {
                        class: "btn btn-secondary",
                        disabled: busy || saved.is_none(),
                        title: if dirty { "Сначала сохранит изменения" } else { "" },
                        onclick: move |_| test_click(true),
                        "Проверить подключение"
                    }
                    if busy {
                        span { class: "notice notice-wait", "Подожди…" }
                    } else if let Some(n) = notice.read().clone() {
                        match n {
                            Notice::Ok(text) => rsx! {
                                span { class: "notice notice-ok", role: "status", {Icon::Check.render(14)} "{text}" }
                            },
                            Notice::Err(text) => rsx! {
                                span { class: "notice notice-err", role: "alert", "{text}" }
                            },
                        }
                    } else if dirty {
                        span { class: "notice", "Есть несохранённые изменения" }
                    }
                }
            }
            if saved.is_some() {
                QueueCard { page }
            }
            p { class: "settings-note",
                "Токен хранится в каталоге данных сервиса (secrets/jira/token, права 0600) и не возвращается в ответах API. "
                "Без настройки плагин ничего не отправляет."
            }
        }
    }
}

/// Есть ли что сохранять: форма отличается от сохранённого или введён токен.
fn is_dirty(saved: Option<&JiraState>, f: &Form) -> bool {
    saved.is_some_and(|s| Form::from_state(s) != *f || !f.token.trim().is_empty())
}

fn save_error_text(e: &ApiError) -> String {
    match e {
        ApiError::Validation(m) => format!("Не сохранено: {m}."),
        other => format!("Не сохранено: {}", error_text(other)),
    }
}

fn test_error_text(e: &ApiError) -> String {
    match e {
        ApiError::Validation(m) | ApiError::PluginUnavailable(m) => m.clone(),
        other => error_text(other),
    }
}

/// Плашка состояния плагина справа в заголовке карточки + очередь.
#[component]
fn PluginBadge(state: Option<JiraState>, plugin: Option<PluginStatusDto>) -> Element {
    let Some(s) = state else {
        return rsx! {};
    };
    let pending = s.queue_pending;
    let delivery_error = plugin
        .as_ref()
        .filter(|p| !p.healthy)
        .and_then(|p| p.last_error.clone().map(|e| (e, p.last_error_at)));
    let (class, title, detail) = if !s.enabled {
        ("badge-state off", "Выключен", None)
    } else if !s.configured {
        ("badge-state off", "Не настроено", s.problem.clone())
    } else if let Some((err, at)) = delivery_error {
        let at = at
            .map(|t| format!(" · {}", fmt::local_hm(t)))
            .unwrap_or_default();
        (
            "badge-state err",
            "Ошибка доставки",
            Some(format!("{err}{at}")),
        )
    } else if let Some(err) = s.last_error.clone() {
        ("badge-state err", "Ошибка", Some(err))
    } else {
        ("badge-state ok", "Подключено", None)
    };
    rsx! {
        div { class: "plugin-state",
            span { class,
                span { class: "badge-dot" }
                "{title}"
            }
            if pending > 0 {
                span { class: "plugin-queue num", title: "Операции, которые ждут отправки в Jira",
                    "в очереди {pending}"
                }
            }
            if let Some(d) = detail {
                span { class: "plugin-detail", title: "{d}", "{d}" }
            }
        }
    }
}

/// Карточка «Очередь отправки»: что уйдёт в Jira, кнопка «Отправить» и
/// удаление ненужного.
#[component]
fn QueueCard(page: Page) -> Element {
    let api = use_api();
    // `Callback` — `Copy`: один обработчик на все строки.
    let api_drop = api.clone();
    let drop_op = use_callback(move |id: i64| {
        let api = api_drop.clone();
        let mut p = page;
        spawn(async move {
            p.queue_busy.set(true);
            match api.jira().drop_jira_op(id).await {
                Ok(q) => {
                    set_queue(p, q);
                    p.queue_notice.set(None);
                }
                Err(e) => p.queue_notice.set(Some(Notice::Err(error_text(&e)))),
            }
            reload(&api, p, false).await;
            p.queue_busy.set(false);
        });
    });

    let Some(q) = page.queue.read().clone() else {
        return rsx! {};
    };
    let configured = page.saved.read().as_ref().is_some_and(|s| s.configured);
    let busy = (page.queue_busy)();
    let n = q.items.len();

    let api_send = api.clone();
    let send = move |_| {
        let api = api_send.clone();
        let mut p = page;
        spawn(async move {
            p.queue_busy.set(true);
            p.queue_notice.set(None);
            let notice = match api.jira().send_jira_queue().await {
                Ok(r) => {
                    let sent = r.sent.unwrap_or(0);
                    let n = Notice::from_send(sent, r.error.clone());
                    set_queue(p, r);
                    n
                }
                Err(e) => Notice::Err(test_error_text(&e)),
            };
            p.queue_notice.set(Some(notice));
            reload(&api, p, false).await;
            p.queue_busy.set(false);
        });
    };

    let hint = if q.auto_send {
        "Автоотправка включена: изменения уходят в Jira сразу, здесь остаются только те, что не прошли."
    } else {
        "В Jira ничего не уходит само: проверь список и нажми «Отправить». Ненужное убери крестиком."
    };

    rsx! {
        section { class: "card settings-card queue-card", "aria-labelledby": "queue-title",
            div { class: "card-head",
                div { class: "card-head-title",
                    h2 { id: "queue-title", class: "card-title", "Очередь отправки" }
                    if n > 0 {
                        span { class: "queue-count num", "{n}" }
                    }
                }
                button {
                    class: "btn btn-accent btn-small",
                    disabled: busy || n == 0 || !configured,
                    title: if configured { "" } else { "Сначала настрой подключение" },
                    onclick: send,
                    "Отправить"
                }
            }
            p { class: "queue-hint", "{hint}" }
            if n == 0 {
                p { class: "queue-empty", "Очередь пуста — всё отправлено." }
            } else {
                ul { class: "queue-list",
                    for op in q.items.iter().cloned() {
                        QueueRow {
                            key: "{op.id}",
                            op: op.clone(),
                            busy,
                            on_drop: move |id| drop_op.call(id),
                        }
                    }
                }
            }
            if busy || page.queue_notice.read().is_some() {
                div { class: "card-foot",
                    if busy {
                        span { class: "notice notice-wait", "Подожди…" }
                    } else if let Some(n) = page.queue_notice.read().clone() {
                        match n {
                            Notice::Ok(text) => rsx! {
                                span { class: "notice notice-ok", role: "status", {Icon::Check.render(14)} "{text}" }
                            },
                            Notice::Err(text) => rsx! {
                                span { class: "notice notice-err", role: "alert", "{text}" }
                            },
                        }
                    }
                }
            }
        }
    }
}

impl Notice {
    fn from_send(sent: u64, error: Option<String>) -> Self {
        let what = |n: u64| {
            format!(
                "{n} {}",
                fmt::plural(n as usize, "запись", "записи", "записей")
            )
        };
        match error {
            None if sent == 0 => Self::Ok("Нечего отправлять.".into()),
            None => Self::Ok(format!("Отправлено: {}.", what(sent))),
            Some(e) if sent == 0 => Self::Err(format!("Не отправлено: {e}")),
            Some(e) => Self::Err(format!("Отправлено {}, дальше ошибка: {e}", what(sent))),
        }
    }
}

/// Одна операция очереди.
#[component]
fn QueueRow(op: JiraOp, busy: bool, on_drop: EventHandler<i64>) -> Element {
    let (icon, label, class) = match op.kind {
        JiraOpKind::ChecklistAdd => (Icon::Plus, "Новый пункт", "queue-kind add"),
        JiraOpKind::ChecklistDone => (Icon::Check, "Пункт готов", "queue-kind done"),
        JiraOpKind::Worklog => (Icon::Play, "Worklog", "queue-kind worklog"),
        JiraOpKind::Unknown => (Icon::External, "Операция", "queue-kind"),
    };
    // Подпись и время (время — моноширинными цифрами).
    let (meta, at) = match (op.started_at, op.seconds) {
        (Some(at), Some(sec)) => (
            format!("{} · начало", fmt::duration(sec.max(60))),
            fmt::local_hm(at),
        ),
        _ => ("в очереди с".to_owned(), fmt::local_hm(op.created_at)),
    };
    let id = op.id;
    rsx! {
        li { class: if op.last_error.is_some() { "queue-row failed" } else { "queue-row" },
            span { class, {icon.render(14)} "{label}" }
            div { class: "queue-main",
                div { class: "queue-text", title: "{op.text}", "{op.text}" }
                div { class: "queue-meta",
                    span { class: "chip chip-jira", "{op.issue_key}" }
                    span { "{meta} " span { class: "num", "{at}" } }
                }
                if let Some(err) = op.last_error.clone() {
                    div { class: "queue-err", title: "{err}",
                        if op.attempts > 1 {
                            "Попыток: {op.attempts} · "
                        }
                        "{err}"
                    }
                }
            }
            button {
                class: "icon-btn",
                disabled: busy,
                title: "Не отправлять",
                "aria-label": "Убрать из очереди: {label} «{op.text}»",
                onclick: move |_| on_drop.call(id),
                {Icon::Close.render(14)}
            }
        }
    }
}

/// Строка формы: подпись слева, поле и подсказка справа.
#[component]
fn FormRow(label: &'static str, hint: &'static str, children: Element) -> Element {
    rsx! {
        div { class: "form-row",
            div { class: "form-label", "{label}" }
            div { class: "form-control",
                {children}
                if !hint.is_empty() {
                    p { class: "form-hint", "{hint}" }
                }
            }
        }
    }
}

/// Переключатель с подписью (вид — как чекбокс задач).
#[component]
fn Toggle(label: &'static str, on: bool, on_toggle: EventHandler<bool>) -> Element {
    rsx! {
        button {
            class: if on { "toggle on" } else { "toggle" },
            role: "checkbox",
            "aria-checked": "{on}",
            onclick: move |_| on_toggle.call(!on),
            span { class: if on { "check checked" } else { "check" },
                if on {
                    {Icon::Check.render(14)}
                }
            }
            "{label}"
        }
    }
}

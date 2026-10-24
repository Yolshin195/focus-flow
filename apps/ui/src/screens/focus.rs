//! Экран «Фокус»: кольцо таймера, пункт, одна главная кнопка по контексту.

use dioxus::prelude::*;
use focus_api_types::{DoneNext, TaskDto, TaskId};

use crate::api::{jira_key, ApiHandle};
use crate::components::{Dialog, Icon, ItemPicker, NormBar, Ring, RingTone, Segment};
use crate::fmt;
use crate::state::{run, use_api, use_app, AppState, Command};
use crate::timer::{use_timer_view, TimerView as View};

/// Найти пункт и его родителя в дереве.
/// Родитель пункта: название и ключ Jira.
type ParentInfo = (String, Option<String>);

fn find_item(tree: &[TaskDto], id: TaskId) -> Option<(String, Option<ParentInfo>)> {
    for t in tree {
        if t.id == id {
            return Some((t.title.clone(), None));
        }
        if let Some(c) = t.children.iter().find(|c| c.id == id) {
            let key = t.external_url.as_deref().and_then(jira_key);
            return Some((c.title.clone(), Some((t.title.clone(), key))));
        }
    }
    None
}

const HOTKEYS_JS: &str = r#"
if (window.__ffKeys) window.removeEventListener('keydown', window.__ffKeys);
window.__ffKeys = (e) => {
    const t = e.target;
    const tag = t && t.tagName;
    const typing = tag === 'INPUT' || tag === 'TEXTAREA' || (t && t.isContentEditable);
    // Пробел и Enter на кнопке нажимают её саму; на ссылке Enter переходит
    // по ней, а Пробел ничего не делает — его забираем (фокус остаётся на
    // пункте навигации после клика).
    const control = tag === 'BUTTON' || (tag === 'A' && e.key === 'Enter');
    if (e.key === 'Escape') { dioxus.send('Escape'); return; }
    if (typing || control || e.repeat || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === ' ' || e.key === 'Enter') {
        e.preventDefault();
        dioxus.send(e.key === ' ' ? 'Space' : 'Enter');
    }
};
window.addEventListener('keydown', window.__ffKeys);
"#;

const HOTKEYS_OFF_JS: &str =
    "if (window.__ffKeys) { window.removeEventListener('keydown', window.__ffKeys); window.__ffKeys = null; }";

/// Глобальные клавиши экрана: Space/Enter — главное действие, Esc — закрыть.
/// Нажатия в полях ввода и на кнопках не перехватываются.
fn use_hotkeys(api: ApiHandle, mut app: AppState) {
    use_future(move || {
        let api = api.clone();
        async move {
            let mut ev = document::eval(HOTKEYS_JS);
            while let Ok(key) = ev.recv::<String>().await {
                if key == "Escape" {
                    if *app.interrupt_dialog.peek() {
                        app.interrupt_dialog.set(false);
                    } else if *app.picker_open.peek() {
                        app.picker_open.set(false);
                    } else if *app.done_choice.peek() {
                        app.done_choice.set(false);
                    }
                } else {
                    main_action(&api, app);
                }
            }
        }
    });
    use_drop(|| {
        document::eval(HOTKEYS_OFF_JS);
    });
}

/// Главное действие экрана для текущего состояния (Space/Enter).
fn main_action(api: &ApiHandle, mut app: AppState) {
    if *app.busy.peek() {
        return;
    }
    if *app.interrupt_dialog.peek() {
        app.interrupt_dialog.set(false);
        return;
    }
    let view = View::of(app.status.peek().as_ref());
    match view {
        View::Idle => {
            if *app.picker_open.peek() {
                return; // Enter в списке обрабатывает сам список.
            }
            match *app.selected_item.peek() {
                Some(id) => start_work(api, app, id),
                None => app.picker_open.set(true),
            }
        }
        View::Work { .. } => {
            if *app.done_choice.peek() {
                finish(api, app, DoneNext::Rest);
            } else {
                app.done_choice.set(true);
            }
        }
        View::Planning { .. } => {
            if !*app.picker_open.peek() {
                app.picker_open.set(true);
            }
        }
        View::Rest { .. } | View::Loading => {}
    }
}

fn start_work(api: &ApiHandle, mut app: AppState, id: TaskId) {
    app.picker_open.set(false);
    run(api.clone(), app, Command::StartWork(id), move || {
        app.selected_item.set(None);
    });
}

fn finish(api: &ApiHandle, mut app: AppState, next: DoneNext) {
    app.done_choice.set(false);
    run(api.clone(), app, Command::Done(next), || {});
}

/// Экран «Фокус».
#[component]
pub fn Focus() -> Element {
    let api = use_api();
    let mut app = use_app();

    use_hotkeys(api.clone(), app);

    let status = app.status.read().clone();
    let view = use_timer_view()();
    let busy = (app.busy)();

    let (worked, target) = status
        .as_ref()
        .map(|s| (s.worked_today_sec, s.target_sec))
        .unwrap_or((0, 8 * 3600));
    let totals = app
        .day
        .read()
        .as_ref()
        .map(|d| d.totals)
        .unwrap_or_default();
    // Активная сессия уже входит в `worked_today_sec`; итоги дня могут отставать на тик.
    let planning_sec = totals.planning_sec.min(worked);
    let segments = vec![
        Segment {
            class: "work",
            sec: worked - planning_sec,
        },
        Segment {
            class: "planning",
            sec: planning_sec,
        },
    ];
    let day_started = status
        .as_ref()
        .and_then(|s| s.day.as_ref())
        .map(|d| fmt::local_hm(d.opened_at));

    // Циферблат: по риске на минуту. Отсчёт вниз — горят оставшиеся минуты,
    // планирование — загораются прошедшие (круг — 50 минут, дальше заново).
    // До данных — тот же циферблат, что в простое, но без цифр и подписей:
    // строки пустые, но той же высоты, поэтому ничего не сдвигается.
    let (label, tone, ticks, lit, digits, sub) = match &view {
        View::Loading => (
            fmt::NBSP,
            RingTone::Idle,
            50,
            1.0,
            fmt::NBSP.to_string(),
            fmt::NBSP,
        ),
        View::Idle => (
            "Готов к работе",
            RingTone::Idle,
            50,
            1.0,
            "50:00".to_string(),
            "один пункт",
        ),
        View::Work {
            remaining, total, ..
        } => (
            "Работа",
            RingTone::Work,
            (*total / 60).max(1) as u32,
            *remaining as f64 / *total as f64,
            fmt::clock(*remaining),
            "осталось",
        ),
        View::Rest { remaining, total } => (
            "Отдых",
            RingTone::Rest,
            (*total / 60).max(1) as u32,
            *remaining as f64 / *total as f64,
            fmt::clock(*remaining),
            "до конца отдыха",
        ),
        View::Planning { elapsed } => (
            "Планирование",
            RingTone::Planning,
            50,
            (*elapsed % 3000) as f64 / 3000.0,
            fmt::clock(*elapsed),
            "идёт в норму",
        ),
    };

    let work_item = view.item().cloned();
    let work_item_key = work_item.as_ref().and_then(|it| {
        app.tasks
            .read()
            .iter()
            .find(|t| Some(t.id) == it.parent_id)
            .and_then(|t| t.external_url.as_deref().and_then(jira_key))
    });

    let selected = app
        .selected_item
        .read()
        .and_then(|id| find_item(&app.tasks.read(), id));

    let lost_min = match (&view, status.as_ref()) {
        (View::Idle, Some(s)) => s
            .idle_since
            .filter(|_| {
                app.day
                    .read()
                    .as_ref()
                    .is_some_and(|d| !d.sessions.is_empty())
            })
            .map(|since| (s.server_time - since).num_minutes().max(0)),
        _ => None,
    };

    // До данных место подсказки и строки «Потеряно» занято (`reserved`), как в
    // простое: когда они появятся, циферблат не сдвинется.
    let loading = view == View::Loading;
    let hint = key_hint(&view, selected.is_some()).or(loading.then_some("выбрать пункт"));
    let lost_min = lost_min.or(loading.then_some(0));

    let api_a = api.clone();
    let api_b = api.clone();
    let api_c = api.clone();
    let api_rest = api.clone();
    let api_d = api.clone();
    let api_e = api.clone();
    let api_f = api.clone();
    let api_g = api.clone();

    rsx! {
        div { class: "focus focus-{tone.class()}",
            header { class: "focus-top",
                div { class: "day-meta",
                    if loading {
                        span { class: "day-started", {fmt::NBSP} }
                    } else if let Some(t) = day_started {
                        span { class: "day-started", "День начат в " span { class: "num", "{t}" } }
                    } else {
                        span { class: "day-started", "День ещё не начат" }
                    }
                }
                div { class: "norm",
                    div { class: "norm-text",
                        if loading {
                            {fmt::NBSP}
                        } else {
                            span { class: "num norm-worked", "{fmt::hm(worked)}" }
                            span { class: "norm-of", " из " }
                            span { class: "num", "{fmt::hm(target)}" }
                        }
                    }
                    NormBar { segments, target_sec: target }
                }
            }

            section { class: "focus-stage", "aria-live": "polite",
                div { class: "session-label", "{label}" }
                Ring { tone, ticks, lit,
                    div { class: "ring-digits num", "{digits}" }
                    div { class: "ring-sub", "{sub}" }
                }

                div { class: "focus-context",
                    match &view {
                        View::Work { .. } => rsx! {
                            if let Some(item) = &work_item {
                                div { class: "item-title", title: "{item.title}", "{item.title}" }
                                if let Some(parent) = &item.parent_title {
                                    div { class: "item-parent",
                                        if let Some(k) = &work_item_key {
                                            span { class: "chip chip-jira", "{k}" }
                                        }
                                        span { class: "item-parent-title", "{parent}" }
                                    }
                                }
                            }
                        },
                        View::Rest { .. } => rsx! {
                            div { class: "rest-hint", "Отдыхай. Встань, пройдись." }
                        },
                        View::Planning { .. } => rsx! {
                            div { class: "planning-hint", "Разложи задачу на пункты и выбери следующий." }
                        },
                        View::Idle => rsx! {
                            button {
                                class: if selected.is_some() { "item-field filled" } else { "item-field" },
                                onclick: move |_| app.picker_open.set(true),
                                if let Some((title, parent)) = &selected {
                                    span { class: "item-field-main",
                                        span { class: "item-field-title", "{title}" }
                                        if let Some((ptitle, key)) = parent {
                                            span { class: "item-parent",
                                                if let Some(k) = key { span { class: "chip chip-jira", "{k}" } }
                                                span { class: "item-parent-title", "{ptitle}" }
                                            }
                                        }
                                    }
                                    span { class: "item-field-change", "Сменить" }
                                } else {
                                    span { class: "item-field-placeholder", "Выбери пункт для работы" }
                                    {Icon::Chevron.render(18)}
                                }
                            }
                        },
                        // Пустое поле того же размера; не кнопка — нажать пока нечего.
                        View::Loading => rsx! {
                            div { class: "item-field", "aria-hidden": "true" }
                        },
                    }
                }

                div { class: "focus-actions",
                    match &view {
                        View::Idle => rsx! {
                            button {
                                class: "btn btn-primary btn-work",
                                disabled: selected.is_none() || busy,
                                onclick: move |_| {
                                    if let Some(id) = *app.selected_item.peek() {
                                        start_work(&api_a, app, id);
                                    }
                                },
                                "Начать 50 минут"
                            }
                            button {
                                class: "btn btn-quiet",
                                disabled: busy,
                                onclick: move |_| run(api_b.clone(), app, Command::StartPlanning, || {}),
                                "Планировать"
                            }
                        },
                        View::Work { .. } => rsx! {
                            button {
                                class: "btn btn-primary btn-work",
                                disabled: busy,
                                onclick: move |_| app.done_choice.set(true),
                                "Готово"
                            }
                            button {
                                class: "btn btn-quiet",
                                disabled: busy,
                                onclick: move |_| app.interrupt_dialog.set(true),
                                "Прервать"
                            }
                        },
                        View::Planning { .. } => rsx! {
                            button {
                                class: "btn btn-primary btn-planning",
                                disabled: busy,
                                onclick: move |_| app.picker_open.set(true),
                                "Начать работу"
                            }
                            div { class: "quiet-row",
                                button {
                                    class: "btn btn-quiet",
                                    disabled: busy,
                                    title: "Закончить планирование и отдохнуть — без потерянного времени",
                                    onclick: move |_| run(api_rest.clone(), app, Command::StartRest, || {}),
                                    "Отдых"
                                }
                                button {
                                    class: "btn btn-quiet",
                                    disabled: busy,
                                    onclick: move |_| run(api_c.clone(), app, Command::Stop, || {}),
                                    "Стоп"
                                }
                            }
                        },
                        // Кнопки простоя, неактивные до данных; «Планировать» только держит место.
                        View::Loading => rsx! {
                            button { class: "btn btn-primary btn-work", disabled: true, "Начать 50 минут" }
                            button { class: "btn btn-quiet reserved", disabled: true, "Планировать" }
                        },
                        View::Rest { .. } => rsx! {},
                    }
                }

                if let Some(hint) = hint {
                    div { class: if loading { "key-hint reserved" } else { "key-hint" },
                        kbd { "Пробел" }
                        span { "{hint}" }
                    }
                }

                if let Some(min) = lost_min {
                    div { class: if loading { "lost-note reserved" } else { "lost-note" },
                        span { class: "lost-dot" }
                        "Потеряно: "
                        span { class: "num", "{min} мин" }
                    }
                }
            }

            if (app.done_choice)() && matches!(view, View::Work { .. }) {
                Dialog { title: "Пункт готов", wide: true, on_close: move |_| app.done_choice.set(false),
                    div { class: "dialog-body",
                        h2 { class: "dialog-title", "Пункт готов" }
                        if let Some(item) = &work_item {
                            p { class: "dialog-text", "«{item.title}» отмечен выполненным. Что дальше?" }
                        }
                    }
                    div { class: "choice",
                        button {
                            class: "choice-card choice-rest",
                            autofocus: true,
                            onclick: move |_| finish(&api_d, app, DoneNext::Rest),
                            {Icon::Rest.render(22)}
                            span { class: "choice-title", "Отдых 10 минут" }
                            span { class: "choice-sub", "Встать, пройтись, отвести взгляд" }
                        }
                        button {
                            class: "choice-card choice-planning",
                            onclick: move |_| finish(&api_e, app, DoneNext::Planning),
                            {Icon::Plan.render(22)}
                            span { class: "choice-title", "Спланировать следующую" }
                            span { class: "choice-sub", "Сразу, без потерянного времени" }
                        }
                    }
                }
            }

            if (app.interrupt_dialog)() && matches!(view, View::Work { .. }) {
                Dialog { title: "Прервать работу?", wide: true, on_close: move |_| app.interrupt_dialog.set(false),
                    div { class: "dialog-body",
                        h2 { class: "dialog-title", "Прервать работу?" }
                        p { class: "dialog-warning", "Ты отвлекаешься. Эти 50 минут тебя ни для кого нет." }
                    }
                    div { class: "dialog-actions",
                        button {
                            class: "btn btn-quiet btn-danger",
                            onclick: move |_| {
                                app.interrupt_dialog.set(false);
                                run(api_f.clone(), app, Command::Interrupt, || {});
                            },
                            "Всё равно прервать"
                        }
                        button {
                            class: "btn btn-primary btn-work",
                            autofocus: true,
                            onclick: move |_| app.interrupt_dialog.set(false),
                            "Вернуться к работе"
                        }
                    }
                }
            }

            if (app.picker_open)() && matches!(view, View::Idle | View::Planning { .. }) {
                Dialog { title: "Над чем работаем?", on_close: move |_| app.picker_open.set(false),
                    div { class: "dialog-body",
                        h2 { class: "dialog-title", "Над чем работаем?" }
                    }
                    ItemPicker {
                        tasks: app.tasks,
                        selected: app.selected_item,
                        on_pick: move |id| start_work(&api_g, app, id),
                    }
                    PickerFooter {}
                }
            }
        }
    }
}

#[component]
fn PickerFooter() -> Element {
    let api = use_api();
    let mut app = use_app();
    let chosen = app.selected_item.read().is_some();
    rsx! {
        div { class: "dialog-actions",
            button {
                class: "btn btn-quiet",
                onclick: move |_| app.picker_open.set(false),
                "Отмена"
            }
            button {
                class: "btn btn-primary btn-work",
                disabled: !chosen || (app.busy)(),
                onclick: move |_| {
                    if let Some(id) = *app.selected_item.peek() {
                        start_work(&api, app, id);
                    }
                },
                "Начать 50 минут"
            }
        }
    }
}

/// Подсказка к главному действию по Пробелу/Enter.
fn key_hint(view: &View, item_chosen: bool) -> Option<&'static str> {
    match view {
        View::Idle if item_chosen => Some("начать"),
        View::Idle => Some("выбрать пункт"),
        View::Work { .. } => Some("готово"),
        View::Planning { .. } => Some("начать работу"),
        View::Rest { .. } | View::Loading => None,
    }
}

//! Трей / строка меню — как «Часы» в строке меню macOS: иконка фазы и время
//! рядом с ней. Левый клик — окно на экране «Фокус», правый — меню команд
//! текущей фазы. Паузы нет ни здесь, ни в окне.
//!
//! Данные — те же, что у окна: [`ui::use_status_line`] из состояния
//! приложения (HttpApi + SSE, время от `ends_at`/`started_at` со сдвигом
//! `server_time`). Своих запросов к сервису и своего таймера у трея нет:
//! раз в 250 мс он сверяет строку с показанной и трогает систему, только
//! если что-то изменилось. Команды идут через тот же `FocusApi`, что и
//! кнопки экрана; «Прервать…» и «Начать работу…» только открывают окно с
//! предупреждением или выбором пункта.
//!
//! Платформы:
//! - macOS — template-иконка (система красит её под светлую и тёмную тему)
//!   и время рядом (`set_title`) моноширинными цифрами;
//! - Linux (AppIndicator) — цветная иконка и подпись (`set_title`; видна не
//!   во всех окружениях). Подсказок и кликов по иконке AppIndicator не
//!   знает: меню открывается любым кликом, окно — пунктом «Открыть»;
//! - Windows — цветная иконка, время — в подсказке (подписей у трея нет).

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::time::Duration;

use dioxus::desktop::tao::event::Event;
use dioxus::desktop::trayicon::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use dioxus::desktop::trayicon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use dioxus::desktop::{
    use_muda_event_handler, use_tray_icon_event_handler, use_tray_menu_event_handler,
    use_wry_event_handler, window, WindowCloseBehaviour,
};
use dioxus::prelude::*;
use focus_api_types::DoneNext;
use ui::state::{run, use_api, use_app, AppState, Command};
use ui::{use_status_line, ApiHandle, Indicator, Reveal, StatusLine};

/// Как часто сверять трей с состоянием.
const TICK: Duration = Duration::from_millis(250);
/// Время рядом с иконкой: у трея Windows подписей нет — там только подсказка.
const SHOW_TITLE: bool = cfg!(not(target_os = "windows"));
/// Иконка-шаблон: на macOS её цвет задаёт система (светлая и тёмная тема).
const TEMPLATE_ICON: bool = cfg!(target_os = "macos");

/// Трей. Живёт внутри `ui::App` (видит состояние приложения), разметки не даёт.
///
/// Если трей создан, закрытие окна только прячет его: состояние, SSE и
/// встроенный сервис работают дальше; полный выход — «Выход» или ⌘Q.
#[component]
pub fn Tray() -> Element {
    let app = use_app();
    let api = use_api();
    let line = use_status_line();

    // Первый рендер — уже внутри цикла событий (на macOS трей раньше нельзя).
    let tray = use_hook(move || {
        let tray = TrayUi::create(&line.peek());
        if tray.is_some() && can_hide_window() {
            window().set_close_behavior(WindowCloseBehaviour::WindowHides);
        }
        tray
    });

    let ticker = tray.clone();
    use_future(move || {
        let tray = ticker.clone();
        async move {
            let Some(tray) = tray else { return };
            loop {
                tray.apply(&line.read());
                futures_timer::Delay::new(TICK).await;
            }
        }
    });

    // Обработчик событий muda один на процесс, и первым его ставит меню
    // окна: в dioxus-desktop 0.7 пункты трея приходят как события меню окна
    // (`use_muda_event_handler`), а не трея. Слушаем оба канала — каждое
    // событие приходит только в один; пункт трея узнаётся по id.
    let menu_api = api.clone();
    use_muda_event_handler(move |ev| on_menu(&ev.id().0, line, app, &menu_api));
    use_tray_menu_event_handler(move |ev| on_menu(&ev.id().0, line, app, &api));

    use_tray_icon_event_handler(move |ev| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = ev
        {
            show_window();
            app.reveal(Reveal::Focus);
        }
    });

    // Клик по иконке в Dock (только macOS), когда окно спрятано.
    use_wry_event_handler(|ev, _| {
        if let Event::Reopen { .. } = ev {
            show_window();
        }
    });

    rsx! {}
}

/// Команда меню трея.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Open,
    DoneRest,
    DonePlanning,
    Interrupt,
    Rest,
    StartRest,
    Stop,
    StartWork,
    Planning,
    Quit,
}

impl Action {
    const ALL: [Action; 10] = [
        Action::Open,
        Action::DoneRest,
        Action::DonePlanning,
        Action::Interrupt,
        Action::Rest,
        Action::StartRest,
        Action::Stop,
        Action::StartWork,
        Action::Planning,
        Action::Quit,
    ];

    /// Id пункта меню (события меню окна и трея приходят одним потоком).
    fn id(self) -> &'static str {
        match self {
            Action::Open => "focus-flow-tray:open",
            Action::DoneRest => "focus-flow-tray:done-rest",
            Action::DonePlanning => "focus-flow-tray:done-planning",
            Action::Interrupt => "focus-flow-tray:interrupt",
            Action::Rest => "focus-flow-tray:rest",
            Action::StartRest => "focus-flow-tray:start-rest",
            Action::Stop => "focus-flow-tray:stop",
            Action::StartWork => "focus-flow-tray:start-work",
            Action::Planning => "focus-flow-tray:planning",
            Action::Quit => "focus-flow-tray:quit",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// Текст пункта — как у кнопок экрана «Фокус».
    fn label(self) -> &'static str {
        match self {
            Action::Open => "Открыть Focus Flow",
            Action::DoneRest => "Отдых 10 минут",
            Action::DonePlanning => "Спланировать следующую",
            Action::Interrupt => "Прервать…",
            Action::Rest => "Отдых",
            Action::StartRest => "Начать отдых",
            Action::Stop => "Стоп",
            Action::StartWork => "Начать работу…",
            Action::Planning => "Планировать",
            Action::Quit => "Выход",
        }
    }

    /// Команда сервису — та же, что у кнопки экрана. У остальных пунктов
    /// команды нет: они открывают окно (или выходят).
    fn command(self) -> Option<Command> {
        match self {
            Action::DoneRest => Some(Command::Done(DoneNext::Rest)),
            Action::DonePlanning => Some(Command::Done(DoneNext::Planning)),
            Action::Rest | Action::StartRest => Some(Command::StartRest),
            Action::Stop => Some(Command::Stop),
            Action::Planning => Some(Command::StartPlanning),
            Action::Open | Action::Interrupt | Action::StartWork | Action::Quit => None,
        }
    }
}

/// Строка меню трея.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Item(Action),
    Submenu(&'static str, Vec<Action>),
    Separator,
}

/// Меню фазы: «Открыть», разделитель, команды фазы, разделитель, «Выход».
fn entries(indicator: Indicator) -> Vec<Entry> {
    let phase = match indicator {
        Indicator::Work => vec![
            Entry::Submenu("Готово", vec![Action::DoneRest, Action::DonePlanning]),
            Entry::Item(Action::Interrupt),
        ],
        Indicator::Planning => vec![
            Entry::Item(Action::Rest),
            Entry::Item(Action::Stop),
            Entry::Item(Action::StartWork),
        ],
        Indicator::Idle => vec![
            Entry::Item(Action::StartWork),
            Entry::Item(Action::Planning),
        ],
        Indicator::RestDue => vec![Entry::Item(Action::StartRest)],
        Indicator::Rest | Indicator::Offline => Vec::new(),
    };
    let mut out = vec![Entry::Item(Action::Open), Entry::Separator];
    if !phase.is_empty() {
        out.extend(phase);
        out.push(Entry::Separator);
    }
    out.push(Entry::Item(Action::Quit));
    out
}

/// Есть ли команда в меню фазы.
fn allowed(action: Action, indicator: Indicator) -> bool {
    entries(indicator).iter().any(|e| match e {
        Entry::Item(a) => *a == action,
        Entry::Submenu(_, items) => items.contains(&action),
        Entry::Separator => false,
    })
}

/// Пункт меню выбран: команда трея, если id наш и пункт ещё к месту.
fn on_menu(id: &str, line: Memo<StatusLine>, app: AppState, api: &ApiHandle) {
    let Some(action) = Action::from_id(id) else {
        return; // пункт меню окна, не трея
    };
    // Меню могло устареть, пока было открыто (например, работа ушла в отдых).
    if allowed(action, line.read().indicator) {
        perform(action, app, api);
    }
}

fn perform(action: Action, app: AppState, api: &ApiHandle) {
    match action {
        Action::Open => {
            show_window();
            app.reveal(Reveal::Focus);
        }
        // Прервать — только через предупреждение в окне, не в один клик.
        Action::Interrupt => {
            show_window();
            app.reveal(Reveal::Interrupt);
        }
        // Без пункта работа не стартует — выбор в окне.
        Action::StartWork => {
            show_window();
            app.reveal(Reveal::StartWork);
        }
        Action::Quit => quit(),
        Action::DoneRest
        | Action::DonePlanning
        | Action::Rest
        | Action::StartRest
        | Action::Stop
        | Action::Planning => {
            // Пока идёт команда, кнопки экрана неактивны — трей тоже ждёт.
            if *app.busy.peek() {
                return;
            }
            if let Some(cmd) = action.command() {
                run(api.clone(), app, cmd, || {});
            }
        }
    }
}

/// Показать окно поверх остальных (из трея, Dock или после сворачивания).
fn show_window() {
    let w = &window().window;
    w.set_visible(true);
    w.set_minimized(false);
    w.set_focus();
}

/// Полный выход: окно закрывается, цикл событий заканчивается, и `on_exit`
/// из [`crate::launch`] останавливает встроенный сервис — как при ⌘Q.
fn quit() {
    let w = window();
    w.set_close_behavior(WindowCloseBehaviour::WindowCloses);
    w.close();
}

/// Можно ли прятать окно при закрытии: вернуть его можно только из трея.
fn can_hide_window() -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::tray_host()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// Трей и то, что в нём уже показано.
#[derive(Clone)]
struct TrayUi {
    icon: TrayIcon,
    /// Одно меню на всё время: на Linux его нельзя заменить, только менять пункты.
    menu: Menu,
    shown: Rc<RefCell<Shown>>,
}

/// Показанное в трее — чтобы не трогать систему без изменений.
#[derive(Default)]
struct Shown {
    indicator: Option<Indicator>,
    title: Option<String>,
    tooltip: Option<String>,
}

impl TrayUi {
    /// Создать трей; `None`, если система его не даёт (тогда всё как без трея).
    fn create(line: &StatusLine) -> Option<Self> {
        let menu = Menu::new();
        fill_menu(&menu, line.indicator);
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(false)
            .with_icon_as_template(TEMPLATE_ICON)
            .with_tooltip(&line.tooltip);
        if let Some(icon) = icon(line.indicator) {
            builder = builder.with_icon(icon);
        }
        // Без библиотеки AppIndicator (Linux) tray-icon паникует, а не
        // возвращает ошибку.
        let icon = match catch_unwind(AssertUnwindSafe(|| builder.build())) {
            Ok(Ok(icon)) => icon,
            Ok(Err(e)) => {
                eprintln!("трей недоступен: {e}");
                return None;
            }
            Err(_) => {
                eprintln!("трей недоступен: нет библиотеки AppIndicator");
                return None;
            }
        };
        #[cfg(target_os = "macos")]
        macos::monospaced_digits(&icon);
        let tray = Self {
            icon,
            menu,
            shown: Rc::new(RefCell::new(Shown {
                indicator: Some(line.indicator),
                title: None,
                tooltip: Some(line.tooltip.clone()),
            })),
        };
        tray.apply(line);
        Some(tray)
    }

    /// Показать строку: иконку и меню — при смене фазы, текст и подсказку —
    /// когда они изменились.
    fn apply(&self, line: &StatusLine) {
        let mut shown = self.shown.borrow_mut();
        if shown.indicator != Some(line.indicator) {
            if let Err(e) = self.set_icon(icon(line.indicator)) {
                eprintln!("иконка трея не обновлена: {e}");
            }
            fill_menu(&self.menu, line.indicator);
            shown.indicator = Some(line.indicator);
        }
        if SHOW_TITLE {
            // Пустая строка, а не `None`: на macOS `None` старый текст не стирает.
            let title = line.title.as_deref().unwrap_or("");
            if shown.title.as_deref() != Some(title) {
                self.icon.set_title(Some(title));
                shown.title = Some(title.to_owned());
            }
        }
        if shown.tooltip.as_deref() != Some(line.tooltip.as_str()) {
            if let Err(e) = self.icon.set_tooltip(Some(&line.tooltip)) {
                eprintln!("подсказка трея не обновлена: {e}");
            }
            shown.tooltip = Some(line.tooltip.clone());
        }
    }

    /// Сменить иконку. На macOS простой `set_icon` сбросил бы режим
    /// template, а `set_icon_with_as_template` на других ОС ничего не делает.
    fn set_icon(&self, icon: Option<Icon>) -> Result<(), dioxus::desktop::trayicon::Error> {
        if TEMPLATE_ICON {
            self.icon.set_icon_with_as_template(icon, true)
        } else {
            self.icon.set_icon(icon)
        }
    }
}

/// Заполнить меню пунктами фазы (старые — убрать).
fn fill_menu(menu: &Menu, indicator: Indicator) {
    while menu.remove_at(0).is_some() {}
    let item = |a: Action| MenuItem::with_id(a.id(), a.label(), true, None);
    for entry in entries(indicator) {
        let added = match entry {
            Entry::Item(a) => menu.append(&item(a)),
            Entry::Separator => menu.append(&PredefinedMenuItem::separator()),
            Entry::Submenu(label, actions) => {
                let sub = Submenu::new(label, true);
                actions
                    .into_iter()
                    .try_for_each(|a| sub.append(&item(a)))
                    .and_then(|()| menu.append(&sub))
            }
        };
        if let Err(e) = added {
            eprintln!("пункт меню трея не добавлен: {e}");
        }
    }
}

/// PNG иконки фазы. На macOS — чёрная template @2x (36 px: tray-icon рисует
/// её высотой 18 pt, на Retina — пиксель в пиксель), на Linux и Windows —
/// в цвете сессии: панель там бывает и светлой, и тёмной.
fn icon_png(indicator: Indicator) -> &'static [u8] {
    macro_rules! png {
        ($name:literal) => {
            if cfg!(target_os = "macos") {
                include_bytes!(concat!("../assets/tray/", $name, "@2x.png")).as_slice()
            } else {
                include_bytes!(concat!("../assets/tray/color/", $name, ".png")).as_slice()
            }
        };
    }
    match indicator {
        Indicator::Work => png!("work"),
        // Время вышло: та же чашка, что у отдыха, — отдых ждёт.
        Indicator::Rest | Indicator::RestDue => png!("rest"),
        Indicator::Planning => png!("planning"),
        Indicator::Idle => png!("idle"),
        Indicator::Offline => png!("offline"),
    }
}

/// Иконка фазы для трея.
fn icon(indicator: Indicator) -> Option<Icon> {
    let decoded = image::load_from_memory_with_format(icon_png(indicator), image::ImageFormat::Png)
        .map_err(|e| e.to_string())
        .and_then(|img| {
            let img = img.into_rgba8();
            let (w, h) = img.dimensions();
            Icon::from_rgba(img.into_raw(), w, h).map_err(|e| e.to_string())
        });
    decoded
        .map_err(|e| eprintln!("иконка трея не загружена: {e}"))
        .ok()
}

#[cfg(target_os = "macos")]
mod macos {
    use dioxus::desktop::trayicon::TrayIcon;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSFont, NSFontWeightRegular};

    /// Цифры одной ширины, как у часов в строке меню: подпись не дёргается
    /// каждую секунду. Размер — системный шрифт строки меню.
    pub fn monospaced_digits(icon: &TrayIcon) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(button) = icon.ns_status_item().and_then(|item| item.button(mtm)) else {
            return;
        };
        let size = NSFont::menuBarFontOfSize(0.0).pointSize();
        // SAFETY: константа AppKit, только чтение.
        let weight = unsafe { NSFontWeightRegular };
        button.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            size, weight,
        )));
    }
}

#[cfg(target_os = "linux")]
mod linux {
    /// Есть ли у панели область значков (StatusNotifierWatcher в сессионной
    /// шине D-Bus). Без неё AppIndicator создаётся, но значка не видно — и
    /// спрятанное окно было бы не вернуть.
    pub fn tray_host() -> bool {
        let check = || -> zbus::Result<bool> {
            let conn = zbus::blocking::Connection::session()?;
            let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
            Ok(dbus.name_has_owner("org.kde.StatusNotifierWatcher".try_into()?)?)
        };
        check().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(indicator: Indicator) -> Vec<Action> {
        entries(indicator)
            .into_iter()
            .flat_map(|e| match e {
                Entry::Item(a) => vec![a],
                Entry::Submenu(_, items) => items,
                Entry::Separator => vec![],
            })
            .collect()
    }

    #[test]
    fn menu_by_phase() {
        use Action::*;
        assert_eq!(
            actions(Indicator::Work),
            [Open, DoneRest, DonePlanning, Interrupt, Quit]
        );
        assert_eq!(
            actions(Indicator::Planning),
            [Open, Rest, Stop, StartWork, Quit]
        );
        assert_eq!(actions(Indicator::Idle), [Open, StartWork, Planning, Quit]);
        assert_eq!(actions(Indicator::Rest), [Open, Quit]);
        assert_eq!(actions(Indicator::RestDue), [Open, StartRest, Quit]);
        assert_eq!(actions(Indicator::Offline), [Open, Quit]);
    }

    #[test]
    fn menu_shape() {
        assert_eq!(
            entries(Indicator::Rest),
            [
                Entry::Item(Action::Open),
                Entry::Separator,
                Entry::Item(Action::Quit)
            ]
        );
        let work = entries(Indicator::Work);
        assert_eq!(
            work[2],
            Entry::Submenu("Готово", vec![Action::DoneRest, Action::DonePlanning])
        );
        assert_eq!(work[work.len() - 2], Entry::Separator);
    }

    #[test]
    fn no_pause_anywhere() {
        for a in Action::ALL {
            assert!(!a.label().to_lowercase().contains("пауз"), "{a:?}");
        }
    }

    #[test]
    fn interrupt_and_start_only_open_window() {
        // «Прервать…» и «Начать работу…» — не команды сервису.
        assert_eq!(Action::Interrupt.command(), None);
        assert_eq!(Action::StartWork.command(), None);
        assert!(Action::Interrupt.label().ends_with('…'));
        assert!(Action::StartWork.label().ends_with('…'));
    }

    #[test]
    fn stale_menu_is_ignored() {
        assert!(allowed(Action::DoneRest, Indicator::Work));
        assert!(!allowed(Action::DoneRest, Indicator::Rest));
        assert!(!allowed(Action::Stop, Indicator::Idle));
        assert!(allowed(Action::Open, Indicator::Offline));
        assert!(allowed(Action::Quit, Indicator::Offline));
    }

    #[test]
    fn ids_round_trip() {
        for a in Action::ALL {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        assert_eq!(Action::from_id("dioxus-float-top"), None);
    }

    #[test]
    fn icons_decode() {
        for ind in [
            Indicator::Work,
            Indicator::Rest,
            Indicator::RestDue,
            Indicator::Planning,
            Indicator::Idle,
            Indicator::Offline,
        ] {
            let img = image::load_from_memory_with_format(icon_png(ind), image::ImageFormat::Png)
                .unwrap()
                .into_rgba8();
            assert_eq!(img.dimensions(), (36, 36), "{ind:?}");
            assert!(img.pixels().any(|p| p.0[3] > 0), "{ind:?}: пустая");
        }
    }
}

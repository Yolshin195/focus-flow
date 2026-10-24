//! Статичный каркас в `index.html` и его стили.
//!
//! Пока качается wasm, страница показывает статичную копию первого кадра
//! приложения; Dioxus заменяет её своим первым кадром. Подмена незаметна,
//! только если разметка совпадает, поэтому каркас не пишется руками:
//!
//! - `skeleton_is_first_frame` рендерит [`web::Root`] до прихода данных
//!   через dioxus-ssr и сравнивает с блоком `<!--skeleton-->…<!--/skeleton-->`.
//!   Обновить каркас после правок оболочки или экрана «Фокус»:
//!   `UPDATE_SKELETON=1 cargo test -p web --test skeleton`.
//! - `critical_css_is_copied_verbatim`: каждое правило `<style id="critical">`
//!   дословно (с точностью до пробелов) есть в `tokens.css` или `main.css`.
//! - `fonts_exist`: шрифты из `index.html` лежат в `apps/ui/assets/fonts`.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use dioxus::history::{provide_history_context, MemoryHistory};
use dioxus::prelude::*;

const START: &str = "<!--skeleton-->";
const END: &str = "<!--/skeleton-->";
const ROOT: &str = r#"<div class="app platform-web">"#;
const BOOT_ROOT: &str = r#"<div class="app platform-web" id="boot" aria-busy="true">"#;

fn web_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn index_path() -> PathBuf {
    web_dir().join("index.html")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Корень для SSR: роутеру нужна история, в браузере её даёт dioxus-web.
#[component]
fn SsrRoot() -> Element {
    use_hook(|| provide_history_context(Rc::new(MemoryHistory::default())));
    rsx! {
        web::Root {}
    }
}

/// Первый кадр приложения: данных ещё нет (опрос `MockApi` не запускался).
fn first_frame() -> String {
    let mut dom = VirtualDom::new(SsrRoot);
    dom.rebuild_in_place();
    dioxus::ssr::render(&dom)
}

/// Первый кадр → разметка для `index.html`: корню — `id="boot"` (его убирает
/// `Root`) и `aria-busy`, ссылкам — base path (`{base_path}` подставляет dx),
/// неразрывным пробелам — `&nbsp;`, чтобы их было видно в файле.
fn as_skeleton(frame: &str) -> String {
    assert!(
        frame.starts_with(ROOT),
        "первый кадр должен начинаться с {ROOT}, а начинается с {}",
        &frame[..frame.len().min(120)]
    );
    frame
        .replacen(ROOT, BOOT_ROOT, 1)
        .replace(r#" href="/"#, r#" href="/{base_path}/"#)
        .replace('\u{a0}', "&nbsp;")
}

#[test]
fn skeleton_is_first_frame() {
    let expected = as_skeleton(&first_frame());
    let path = index_path();
    let page = read(&path);
    let (head, rest) = page
        .split_once(START)
        .unwrap_or_else(|| panic!("в {} нет маркера {START}", path.display()));
    let (current, tail) = rest
        .split_once(END)
        .unwrap_or_else(|| panic!("в {} нет маркера {END}", path.display()));
    if current == expected {
        return;
    }
    if std::env::var_os("UPDATE_SKELETON").is_some() {
        std::fs::write(&path, format!("{head}{START}{expected}{END}{tail}"))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    panic!(
        "каркас в apps/web/index.html не совпадает с первым кадром приложения.\n\
         Обновить: UPDATE_SKELETON=1 cargo test -p web --test skeleton\n\
         и проверить, что critical CSS покрывает новую разметку.\n\n\
         ожидается:\n{expected}\n\nсейчас:\n{current}"
    );
}

/// Текст между `open` и следующим `close`.
fn between<'a>(s: &'a str, open: &str, close: &str) -> &'a str {
    let start = s.find(open).unwrap_or_else(|| panic!("не найдено {open}")) + open.len();
    let len = s[start..]
        .find(close)
        .unwrap_or_else(|| panic!("не найдено {close} после {open}"));
    &s[start..start + len]
}

/// CSS без комментариев и с одним пробелом там, где их было несколько; без
/// пробелов вокруг `{ } ; ,` — чтобы сравнивать правила, а не форматирование.
fn normalize(css: &str) -> String {
    let mut out = String::new();
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = rest[i..].find("*/").map_or("", |j| &rest[i + j + 2..]);
    }
    out.push_str(rest);
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s = collapsed;
    for c in ["{", "}", ";", ","] {
        s = s.replace(&format!(" {c}"), c).replace(&format!("{c} "), c);
    }
    s.replace(";}", "}")
}

/// Правила верхнего уровня (вместе с `@media { … }`).
fn top_level_rules(css: &str) -> Vec<String> {
    let mut rules = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in css.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    rules.push(css[start..=i].to_owned());
                    start = i + 1;
                }
            }
            _ => {}
        }
    }
    rules
}

#[test]
fn critical_css_is_copied_verbatim() {
    let page = read(&index_path());
    let assets = web_dir().join("../ui/assets");
    let source = normalize(&format!(
        "{}\n{}",
        read(&assets.join("tokens.css")),
        read(&assets.join("main.css"))
    ));
    let critical = normalize(between(&page, r#"<style id="critical">"#, "</style>"));
    let rules = top_level_rules(&critical);
    assert!(!rules.is_empty(), "critical CSS пуст");
    for rule in rules {
        assert!(
            source.contains(&rule),
            "правила из critical CSS (apps/web/index.html) нет в tokens.css/main.css — \
             скопируйте его заново:\n{rule}"
        );
    }
}

#[test]
fn fonts_exist() {
    let page = read(&index_path());
    let fonts = web_dir().join("../ui/assets/fonts");
    let mut seen = 0;
    for part in page.split("assets/fonts/").skip(1) {
        let name = &part[..part.find(".woff2").expect("шрифт без .woff2") + 6];
        assert!(fonts.join(name).is_file(), "нет шрифта {name}");
        seen += 1;
    }
    assert!(seen > 0, "в index.html нет шрифтов");
}

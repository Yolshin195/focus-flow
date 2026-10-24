//! Шрифты в комплекте (OFL): Inter — интерфейс, JetBrains Mono — таймеры.
//!
//! Папка `assets/fonts` копируется как есть (без хэшей), а `@font-face`
//! собирается здесь — относительные `url()` в CSS-ассете после хэширования
//! указывали бы в никуда. Сетевых шрифтов нет.

use dioxus::prelude::*;

const FONTS: Asset = asset!("/assets/fonts", AssetOptions::folder());

const CYRILLIC: &str = "U+0301,U+0400-045F,U+0490-0491,U+04B0-04B1,U+2116";
const LATIN: &str = "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,\
U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD";

/// CSS с `@font-face` для всех начертаний.
pub fn font_faces() -> String {
    let base = FONTS.to_string();
    let mut css = String::new();
    for w in [400, 500, 600] {
        for (sub, range) in [("cyrillic", CYRILLIC), ("latin", LATIN)] {
            css.push_str(&format!(
                "@font-face{{font-family:\"Inter\";font-style:normal;font-weight:{w};\
                 font-display:block;src:url(\"{base}/inter-{sub}-{w}-normal.woff2\") format(\"woff2\");\
                 unicode-range:{range};}}\n"
            ));
        }
    }
    for w in [300, 400, 500] {
        css.push_str(&format!(
            "@font-face{{font-family:\"JetBrains Mono\";font-style:normal;font-weight:{w};\
             font-display:block;src:url(\"{base}/jetbrains-mono-latin-{w}-normal.woff2\") format(\"woff2\");}}\n"
        ));
    }
    css
}

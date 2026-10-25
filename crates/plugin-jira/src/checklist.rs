//! Текст Smart Checklist (Railsware): строка на пункт, маркер в начале —
//! `-` todo, `+` done, `~` in progress, `x` skipped. Строки `#…` —
//! заголовки, `>…` — подробности предыдущего пункта; пустые строки
//! пропускаются. У строк нет id: пункт ищется по тексту.

use focus_api_types::TaskStatus;
use focus_core::ImportedItem;

/// Маркер строки чек-листа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// `-` — сделать.
    Todo,
    /// `+` — сделано.
    Done,
    /// `~` — в работе.
    InProgress,
    /// `x` — пропущено.
    Skipped,
}

impl Marker {
    fn from_char(c: char) -> Option<Self> {
        match c {
            '-' => Some(Self::Todo),
            '+' => Some(Self::Done),
            '~' => Some(Self::InProgress),
            'x' | 'X' => Some(Self::Skipped),
            _ => None,
        }
    }

    /// Статус пункта у нас.
    pub fn status(self) -> TaskStatus {
        match self {
            Self::Todo => TaskStatus::Planned,
            Self::Done => TaskStatus::Done,
            Self::InProgress => TaskStatus::InProgress,
            Self::Skipped => TaskStatus::Cancelled,
        }
    }
}

/// Строка-пункт чек-листа.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line<'a> {
    /// Маркер; `None` — строка без маркера (считается todo).
    pub marker: Option<Marker>,
    /// Текст после маркера, без пробелов по краям (как в Jira).
    pub text: &'a str,
}

impl Line<'_> {
    /// Статус пункта у нас.
    pub fn status(&self) -> TaskStatus {
        self.marker.map_or(TaskStatus::Planned, Marker::status)
    }
}

/// Разобрать строку; `None` — не пункт (пустая, заголовок, подробности).
pub fn parse_line(line: &str) -> Option<Line<'_>> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') || t.starts_with('>') {
        return None;
    }
    let mut chars = t.chars();
    let first = chars.next()?;
    let rest = chars.as_str();
    let line = match Marker::from_char(first) {
        // Маркер — только с пробелом после него («xml» — не пропущенный пункт).
        Some(m) if rest.is_empty() || rest.starts_with(char::is_whitespace) => Line {
            marker: Some(m),
            text: rest.trim(),
        },
        _ => Line {
            marker: None,
            text: t,
        },
    };
    (!line.text.is_empty()).then_some(line)
}

/// Убрать простое markdown-выделение: `**`, `__`, `~~`, обрамляющие `*`/`_`/`` ` ``.
pub fn clean(text: &str) -> String {
    let mut s = text.replace("**", "").replace("__", "").replace("~~", "");
    loop {
        let t = s.trim();
        let wrapped = ['*', '_', '`']
            .iter()
            .any(|&c| t.len() >= 2 && t.starts_with(c) && t.ends_with(c));
        if !wrapped {
            break;
        }
        s = t[1..t.len() - 1].to_owned();
    }
    s.trim().to_owned()
}

/// Пункты чек-листа в порядке строк.
pub fn items(text: &str) -> Vec<ImportedItem> {
    text.lines()
        .filter_map(parse_line)
        .filter_map(|l| {
            let title = clean(l.text);
            (!title.is_empty()).then(|| ImportedItem {
                title,
                status: l.status(),
            })
        })
        .collect()
}

/// Номер строки (в `text.split('\n')`) пункта с текстом `target`: сначала
/// точное совпадение (после маркера и выделения), затем без учёта регистра.
pub fn find_line(text: &str, target: &str) -> Option<usize> {
    let target = clean(target.trim());
    let lines: Vec<(usize, String)> = text
        .split('\n')
        .enumerate()
        .filter_map(|(i, l)| parse_line(l).map(|p| (i, clean(p.text))))
        .collect();
    lines
        .iter()
        .find(|(_, t)| *t == target)
        .or_else(|| {
            let low = target.to_lowercase();
            lines.iter().find(|(_, t)| t.to_lowercase() == low)
        })
        .map(|(i, _)| *i)
}

/// Отметить строку `idx` как сделанную (`+`). `None` — уже `+` или не пункт.
/// Остальной текст (другие строки, переводы строк) не меняется.
pub fn set_done(text: &str, idx: usize) -> Option<String> {
    set_marker(text, idx, |m| m != Some(Marker::Done), '+')
}

/// Снять отметку со строки `idx` (`+` → `-`). `None` — строка и так не
/// сделана (`-`, `~`, `x`, без маркера) или не пункт.
pub fn set_todo(text: &str, idx: usize) -> Option<String> {
    set_marker(text, idx, |m| m == Some(Marker::Done), '-')
}

/// Поставить маркер `to` строке `idx`, если `change(маркер)`.
fn set_marker(
    text: &str,
    idx: usize,
    change: impl Fn(Option<Marker>) -> bool,
    to: char,
) -> Option<String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let line = lines.get_mut(idx)?;
    let parsed = parse_line(line)?;
    if !change(parsed.marker) {
        return None;
    }
    let indent_len = line.len() - line.trim_start().len();
    let (indent, body) = line.split_at(indent_len);
    let new = match parsed.marker {
        Some(_) => {
            let mut chars = body.chars();
            chars.next();
            format!("{indent}{to}{}", chars.as_str())
        }
        None => format!("{indent}{to} {body}"),
    };
    *line = new;
    Some(lines.join("\n"))
}

/// Статус пункта по строке `idx` (`None` — не пункт).
pub fn status_at(text: &str, idx: usize) -> Option<TaskStatus> {
    text.split('\n')
        .nth(idx)
        .and_then(parse_line)
        .map(|l| l.status())
}

/// Дописать пункт `- title` в конец.
pub fn append(text: &str, title: &str) -> String {
    let title = title.trim();
    if text.trim().is_empty() {
        return format!("- {title}");
    }
    let sep = if text.ends_with('\n') { "" } else { "\n" };
    format!("{text}{sep}- {title}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Реалистичный чек-лист (из задачи шага 7).
    pub const SAMPLE: &str = "- Вердикт в PDF и в чате считается по разным правилам\n- (1) Название отчёта по тестированию — с временем\n+ (30) Пароль не выводить в историю\n~ (31) Останавливать прогон при сбое\nx Одна задача за запуск\n## Заметки\n- Подключить роутер llm ноду";

    #[test]
    fn parses_sample() {
        let it = items(SAMPLE);
        let got: Vec<(&str, TaskStatus)> =
            it.iter().map(|i| (i.title.as_str(), i.status)).collect();
        assert_eq!(
            got,
            [
                (
                    "Вердикт в PDF и в чате считается по разным правилам",
                    TaskStatus::Planned
                ),
                (
                    "(1) Название отчёта по тестированию — с временем",
                    TaskStatus::Planned
                ),
                ("(30) Пароль не выводить в историю", TaskStatus::Done),
                ("(31) Останавливать прогон при сбое", TaskStatus::InProgress),
                ("Одна задача за запуск", TaskStatus::Cancelled),
                ("Подключить роутер llm ноду", TaskStatus::Planned),
            ]
        );
    }

    #[test]
    fn markers_need_space_and_text() {
        assert_eq!(parse_line("xml-парсер").unwrap().marker, None);
        assert_eq!(parse_line("xml-парсер").unwrap().text, "xml-парсер");
        assert_eq!(parse_line("  -   отступ ").unwrap().text, "отступ");
        assert!(parse_line("-").is_none());
        assert!(parse_line("   ").is_none());
        assert!(parse_line("# Заголовок").is_none());
        assert!(parse_line("> подробности").is_none());
        assert_eq!(
            parse_line("X готово?").unwrap().marker,
            Some(Marker::Skipped)
        );
    }

    #[test]
    fn cleans_emphasis() {
        assert_eq!(clean("**важно** сделать"), "важно сделать");
        assert_eq!(clean("*курсив*"), "курсив");
        assert_eq!(clean("`код`"), "код");
        assert_eq!(clean("~~зачёркнуто~~ нет"), "зачёркнуто нет");
        assert_eq!(clean("snake_case_name"), "snake_case_name");
    }

    #[test]
    fn finds_exact_then_case_insensitive() {
        assert_eq!(find_line(SAMPLE, "Одна задача за запуск"), Some(4));
        assert_eq!(find_line(SAMPLE, "подключить РОУТЕР llm ноду"), Some(6));
        assert_eq!(find_line(SAMPLE, "Заметки"), None);
        assert_eq!(find_line(SAMPLE, "нет такого"), None);
        // Точное совпадение важнее регистронезависимого.
        let t = "- abc\n- ABC";
        assert_eq!(find_line(t, "ABC"), Some(1));
    }

    #[test]
    fn set_done_keeps_rest() {
        let t = "- a\r\n  ~ b\r\nc\n+ d";
        assert_eq!(set_done(t, 0).unwrap(), "+ a\r\n  ~ b\r\nc\n+ d");
        assert_eq!(set_done(t, 1).unwrap(), "- a\r\n  + b\r\nc\n+ d");
        assert_eq!(set_done(t, 2).unwrap(), "- a\r\n  ~ b\r\n+ c\n+ d");
        assert_eq!(set_done(t, 3), None);
        assert_eq!(set_done(t, 9), None);
    }

    #[test]
    fn set_todo_only_unchecks_done() {
        let t = "+ a\r\n  + b\n- c\n~ d\nx e";
        assert_eq!(set_todo(t, 0).unwrap(), "- a\r\n  + b\n- c\n~ d\nx e");
        assert_eq!(set_todo(t, 1).unwrap(), "+ a\r\n  - b\n- c\n~ d\nx e");
        for i in [2, 3, 4, 9] {
            assert_eq!(set_todo(t, i), None, "{i}");
        }
        assert_eq!(status_at(t, 3), Some(TaskStatus::InProgress));
        assert_eq!(status_at(t, 9), None);
    }

    #[test]
    fn appends_line() {
        assert_eq!(append("", "a"), "- a");
        assert_eq!(append("- x", " a "), "- x\n- a");
        assert_eq!(append("- x\n", "a"), "- x\n- a");
    }
}

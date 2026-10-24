//! Форматирование времени и длительностей для интерфейса.

use focus_api_types::chrono::{DateTime, Datelike, Local, NaiveDate, Utc, Weekday};

/// Таймер: `MM:SS`, от часа — `H:MM:SS`.
pub fn clock(sec: i64) -> String {
    let sec = sec.max(0);
    let (h, m, s) = (sec / 3600, sec % 3600 / 60, sec % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Часы и минуты: `5:20`.
pub fn hm(sec: i64) -> String {
    let min = sec.max(0) / 60;
    format!("{}:{:02}", min / 60, min % 60)
}

/// Длительность словами: `1 ч 05 мин`, `12 мин`, `40 с`.
pub fn duration(sec: i64) -> String {
    let sec = sec.max(0);
    let min = sec / 60;
    match (min / 60, min % 60) {
        (0, 0) => format!("{sec} с"),
        (0, m) => format!("{m} мин"),
        (h, 0) => format!("{h} ч"),
        (h, m) => format!("{h} ч {m:02} мин"),
    }
}

/// Местное время `09:12`.
pub fn local_hm(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%H:%M").to_string()
}

/// Сегодняшняя дата в местном поясе для момента `t`.
pub fn local_date(t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&Local).date_naive()
}

/// `Суббота, 24 октября`.
pub fn long_date(d: NaiveDate) -> String {
    const MONTHS: [&str; 12] = [
        "января",
        "февраля",
        "марта",
        "апреля",
        "мая",
        "июня",
        "июля",
        "августа",
        "сентября",
        "октября",
        "ноября",
        "декабря",
    ];
    let wd = match d.weekday() {
        Weekday::Mon => "Понедельник",
        Weekday::Tue => "Вторник",
        Weekday::Wed => "Среда",
        Weekday::Thu => "Четверг",
        Weekday::Fri => "Пятница",
        Weekday::Sat => "Суббота",
        Weekday::Sun => "Воскресенье",
    };
    let month = MONTHS.get(d.month0() as usize).copied().unwrap_or("");
    format!("{wd}, {} {month}", d.day())
}

/// Русское окончание: 1 пункт, 2 пункта, 5 пунктов.
pub fn plural<'a>(n: usize, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let (n10, n100) = (n % 10, n % 100);
    if n10 == 1 && n100 != 11 {
        one
    } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
        few
    } else {
        many
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(clock(50 * 60), "50:00");
        assert_eq!(clock(3725), "1:02:05");
        assert_eq!(hm(5 * 3600 + 20 * 60 + 59), "5:20");
        assert_eq!(duration(65 * 60), "1 ч 05 мин");
        assert_eq!(duration(12 * 60), "12 мин");
        assert_eq!(plural(21, "пункт", "пункта", "пунктов"), "пункт");
        assert_eq!(plural(12, "пункт", "пункта", "пунктов"), "пунктов");
        assert_eq!(plural(3, "пункт", "пункта", "пунктов"), "пункта");
    }
}

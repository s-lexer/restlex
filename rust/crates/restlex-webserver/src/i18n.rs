//! Локализация веб-интерфейса отчётов (RU/EN) без внешних зависимостей.
//!
//! Устройство:
//! * Исходный (русский) текст остаётся в коде как есть. `t(lang, ru)` для RU
//!   возвращает строку без изменений, для EN — переводит её по таблице фраз
//!   [`PHRASES`] (поиск подстрок с проверкой границ слова, длинные фразы раньше
//!   коротких). Результат кешируется: вход — только `&'static str`, поэтому
//!   кеш ограничен числом литералов в коде.
//! * Шаблоны страниц переводятся ЦЕЛИКОМ до подстановки данных, а данные
//!   (профили, хосты, ошибки из отчётов агентов) подставляются уже после —
//!   через [`fill`]. Так пользовательские данные никогда не «переводятся».
//! * Язык: cookie `lang` (ru|en), иначе Accept-Language.

use axum::http::{header, HeaderMap};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang { Ru, En }

impl Lang {
    pub fn parse(s: &str) -> Option<Lang> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ru" => Some(Lang::Ru),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self { Lang::Ru => "ru", Lang::En => "en" }
    }

    /// cookie `lang` → Accept-Language → RU (заголовка нет — как раньше).
    pub fn from_headers(h: &HeaderMap) -> Lang {
        if let Some(l) = h.get_all(header::COOKIE).iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|kv| kv.trim().split_once('='))
            .filter(|(k, _)| k.trim() == "lang")
            .find_map(|(_, v)| Lang::parse(v))
        {
            return l;
        }
        match h.get(header::ACCEPT_LANGUAGE).and_then(|v| v.to_str().ok()) {
            Some(al) => from_accept_language(al),
            None => Lang::Ru,
        }
    }

    /// Set-Cookie для переключателя языка (год, весь сайт).
    pub fn cookie(self, secure: bool) -> String {
        let sec = if secure { "; Secure" } else { "" };
        format!("lang={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000{sec}", self.code())
    }
}

/// Язык с наибольшим q; ru/be/uk/kk → RU, всё остальное → EN.
pub fn from_accept_language(al: &str) -> Lang {
    let mut best: Option<(f32, &str)> = None;
    for item in al.split(',') {
        let mut parts = item.split(';');
        let tag = parts.next().unwrap_or("").trim();
        if tag.is_empty() || tag == "*" { continue; }
        let q = parts
            .filter_map(|p| p.trim().strip_prefix("q="))
            .find_map(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(1.0);
        if q <= 0.0 { continue; }
        if best.is_none_or(|(bq, _)| q > bq) { best = Some((q, tag)); }
    }
    let primary = best.map(|(_, tag)| tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase());
    match primary.as_deref() {
        Some("ru" | "be" | "uk" | "kk") => Lang::Ru,
        _ => Lang::En,
    }
}

/// Перевод статического текста/шаблона. Для RU — без изменений.
pub fn t(lang: Lang, ru: &'static str) -> &'static str {
    if lang == Lang::Ru { return ru; }
    static CACHE: OnceLock<RwLock<HashMap<(usize, usize), &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    let key = (ru.as_ptr() as usize, ru.len());
    if let Some(s) = cache.read().get(&key) { return s; }
    let translated = translate_en(ru);
    let out: &'static str = if translated == ru { ru } else { Box::leak(translated.into_boxed_str()) };
    cache.write().entry(key).or_insert(out)
}

fn is_cyr(c: char) -> bool {
    matches!(c, 'А'..='я' | 'Ё' | 'ё')
}

fn sorted_phrases() -> &'static [(&'static str, &'static str)] {
    static SORTED: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    SORTED.get_or_init(|| {
        let mut v = PHRASES.to_vec();
        v.sort_by_key(|p| std::cmp::Reverse(p.0.chars().count()));
        v
    })
}

/// Замена фраз с проверкой границ: фраза, начинающаяся (кончающаяся) русской
/// буквой, не заменяется внутри более длинного слова («Все» в «Всего»).
fn translate_en(src: &str) -> String {
    let mut s = src.to_string();
    for (ru, en) in sorted_phrases() {
        if !s.contains(ru) { continue; }
        let first_cyr = ru.chars().next().is_some_and(is_cyr);
        let last_cyr = ru.chars().last().is_some_and(is_cyr);
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        let mut prev: Option<char> = None;
        while let Some(i) = rest.find(ru) {
            let before = rest[..i].chars().last().or(if i == 0 { prev } else { None });
            let after = rest[i + ru.len()..].chars().next();
            let ok = !(first_cyr && before.is_some_and(is_cyr)) && !(last_cyr && after.is_some_and(is_cyr));
            out.push_str(&rest[..i]);
            if ok { out.push_str(en); } else { out.push_str(ru); }
            prev = ru.chars().last();
            rest = &rest[i + ru.len()..];
        }
        out.push_str(rest);
        s = out;
    }
    s
}

/// Подстановка `{name}` из `vars` (после перевода шаблона). `{{`/`}}` → `{`/`}`
/// — шаблоны раньше были строками `format!`. Значения повторно не сканируются,
/// так что фигурные скобки в данных безопасны.
pub fn fill(tpl: &str, vars: &[(&str, &str)]) -> String {
    let extra: usize = vars.iter().map(|(_, v)| v.len()).sum();
    let mut out = String::with_capacity(tpl.len() + extra);
    let mut rest = tpl;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let r = &rest[i..];
        if let Some(tail) = r.strip_prefix("{{") { out.push('{'); rest = tail; continue; }
        if let Some(tail) = r.strip_prefix("}}") { out.push('}'); rest = tail; continue; }
        if let Some(body) = r.strip_prefix('{') {
            if let Some(end) = body.find('}') {
                let name = &body[..end];
                if let Some((_, v)) = vars.iter().find(|(k, _)| *k == name) {
                    out.push_str(v);
                    rest = &body[end + 1..];
                    continue;
                }
                debug_assert!(
                    name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                    "fill: нет значения для {{{name}}}"
                );
            }
        }
        out.push_str(&r[..1]);
        rest = &r[1..];
    }
    out.push_str(rest);
    out
}

/// (русский текст, английский). Ключи — фразы ровно так, как они стоят в
/// шаблонах web_server.rs (включая `{плейсхолдеры}`).
static PHRASES: &[(&str, &str)] = &[
    ("<html lang=ru>", "<html lang=en>"),
    ("'ru-RU'", "'en-GB'"),
    // вход
    ("RestLex — Вход", "RestLex — Sign in"),
    ("RestLex — вход", "RestLex — sign in"),
    ("Логин", "Username"),
    ("Пароль", "Password"),
    ("Войти", "Sign in"),
    ("Слишком много неудачных попыток. Вход с вашего IP заблокирован ещё на ~{mins} мин.",
     "Too many failed attempts. Sign-in from your IP is blocked for another ~{mins} min."),
    ("Сервер занят проверкой паролей. Повторите через несколько секунд.",
     "The server is busy verifying passwords. Please retry in a few seconds."),
    ("Неверный логин или пароль", "Invalid username or password"),
    // меню
    ("Админка", "Admin"),
    ("Обновления", "Updates"),
    ("Версии", "Versions"),
    ("Тема интерфейса", "Interface theme"),
    ("Тема", "Theme"),
    ("Светлая", "Light"),
    ("Графит", "Graphite"),
    ("Выйти", "Sign out"),
    ("Главная", "Home"),
    ("Язык", "Language"),
    // flash
    ("Настройки сохранены", "Settings saved"),
    ("Группа добавлена", "Group added"),
    ("Группа удалена", "Group deleted"),
    ("Telegram — сообщение отправлено", "Telegram — message sent"),
    ("Telegram — ошибка отправки", "Telegram — sending failed"),
    ("Заполните Bot Token и Chat ID", "Fill in Bot Token and Chat ID"),
    ("Блокировки сняты", "Lockouts cleared"),
    ("Ошибка", "Error"),
    // статусы / модалка / JS
    ("просрочен", "overdue"),
    ("Прерван", "Cancelled"),
    ("Нажмите для деталей", "Click for details"),
    ("Детали", "Details"),
    ("Закрыть", "Close"),
    ("Сбойных файлов: ", "Failed files: "),
    ("(нет деталей)", "(no details)"),
    ("Не удалось сохранить состояние групп", "Failed to save group state"),
    ("Развернуть группы", "Expand groups"),
    ("Свернуть группы", "Collapse groups"),
    ("'проблема':count<5?'проблемы':'проблем'", "'problem':count<5?'problems':'problems'"),
    ("Показано ", "Shown "),
    ("' из '", "' of '"),
    ("Нет записей об обновлениях", "No update records"),
    ("Ошибок обновления нет", "No update errors"),
    ("Успешных обновлений нет", "No successful updates"),
    ("Не удалось сохранить тему", "Failed to save theme"),
    ("Обновлено в ", "Updated at "),
    ("Сессия истекла — данные на странице устарели.", "Session expired — the data on this page is outdated."),
    ("Войти заново", "Sign in again"),
    ("Обновление страницы:", "Page refresh:"),
    ("Автосохранено", "Autosaved"),
    ("Ошибка сохранения", "Save error"),
    ("Версия отличается от опубликованной", "Version differs from the published release"),
    ("['Б','КБ','МБ','ГБ','ТБ']", "['B','KB','MB','GB','TB']"),
    ("' ПБ'", "' PB'"),
    ("'с'", "'s'"),
    ("'м '", "'m '"),
    ("'м'", "'m'"),
    ("'ч '", "'h '"),
    // таблица отчётов
    ("Профиль", "Profile"),
    ("Хост", "Host"),
    ("Статус", "Status"),
    ("Начало", "Started"),
    ("Длит.", "Dur."),
    ("Добавлено", "Added"),
    ("Источник", "Source"),
    ("Файлов", "Files"),
    ("Свежесть", "Freshness"),
    ("Группа", "Group"),
    ("Удалить эту запись?", "Delete this record?"),
    ("Удалить эту запись", "Delete this record"),
    ("Удалить запись?", "Delete record?"),
    ("Удалить запись", "Delete record"),
    ("История", "History"),
    // главная
    ("RestLex — Отчёты", "RestLex — Reports"),
    ("Без группы", "Ungrouped"),
    ("Нет данных. Настройте report_url в профилях RestLex.", "No data. Configure report_url in RestLex profiles."),
    ("Фильтры отчётов", "Report filters"),
    ("Показать:", "Show:"),
    ("Все", "All"),
    ("Ошибки, прерванные и просроченные бэкапы", "Errors, cancelled and overdue backups"),
    ("Проблемные", "Problems"),
    ("Ошибки", "Errors"),
    ("Просроченные", "Overdue"),
    ("Десять профилей с самой старой копией", "Ten profiles with the oldest backup"),
    ("Самые старые · 10", "Oldest · 10"),
    // история
    ("Назад к отчётам", "Back to reports"),
    ("Последние {lim} бэкапов", "Last {lim} backups"),
    ("Удалить всю историю этого бэкапа", "Delete the entire history of this backup"),
    ("Удалить ВСЮ историю бэкапа «{p}»?", "Delete the ENTIRE history of backup “{p}”?"),
    ("Удалить всё", "Delete all"),
    // админка: группы / пользователи / блокировки
    ("авто", "auto"),
    ("ручная", "manual"),
    ("Удалить группу?", "Delete group?"),
    ("Нет групп", "No groups"),
    ("новый пароль", "new password"),
    ("Сменить пароль", "Change password"),
    ("Переключить роль пользователя {u}?", "Toggle the role of user {u}?"),
    ("Переключить роль", "Toggle role"),
    ("Удалить пользователя {u}?", "Delete user {u}?"),
    ("Нет пользователей", "No users"),
    ("не установлен", "not set"),
    ("установлен", "set"),
    ("не настроен", "not configured"),
    ("настроен", "configured"),
    ("заблокирован до {until}", "locked until {until}"),
    ("попытки (не заблокирован)", "attempts (not locked)"),
    ("Разблокировать всех", "Unlock all"),
    ("Разблокировать", "Unlock"),
    ("Нет заблокированных IP и неудачных попыток входа", "No locked IPs or failed sign-in attempts"),
    ("Локаут выключен (0 попыток)", "Lockout disabled (0 attempts)"),
    ("{a} неудачных попыток → блок на {b} мин", "{a} failed attempts → {b} min lock"),
    // админка: вкладки и настройки
    ("Настройки", "Settings"),
    ("Пользователи", "Users"),
    ("Группы", "Groups"),
    ("Блокировки входа", "Sign-in lockouts"),
    ("Блокировки", "Lockouts"),
    ("Доступ", "Access"),
    ("Логин администратора", "Administrator username"),
    ("Новый пароль администратора", "New administrator password"),
    ("Пароль для просмотра", "Viewer password"),
    ("(оставить пустым — не менять)", "(leave empty to keep)"),
    ("Отображение", "Display"),
    ("История (бэкапов на профиль)", "History (backups per profile)"),
    ("штук", "items"),
    ("Авто-группировка по префиксу", "Auto-grouping by prefix"),
    ("Профиль «PREFIX-что-угодно» → группа «PREFIX». Ручные группы с regex имеют приоритет.",
     "Profile “PREFIX-anything” → group “PREFIX”. Manual regex groups take priority."),
    ("Устаревание бэкапа (часов)", "Backup staleness (hours)"),
    ("0 = выключено", "0 = disabled"),
    ("Telegram-уведомления", "Telegram notifications"),
    ("Убрать токен", "Remove token"),
    ("ID темы (topic)", "Topic ID"),
    ("message_thread_id — тема внутри чата выше. Пусто = General.",
     "message_thread_id — topic inside the chat above. Empty = General."),
    ("Безопасность", "Security"),
    ("API ключ", "API key"),
    ("Агенты передают в заголовке", "Agents send it in the header"),
    ("Убрать API ключ", "Remove API key"),
    ("Локаут: неудачных попыток", "Lockout: failed attempts"),
    ("0 = выключить блокировку входа", "0 = disable sign-in lockout"),
    ("Локаут: блокировка (минут)", "Lockout: lock duration (minutes)"),
    ("Сохранить настройки", "Save settings"),
    ("Проверить Telegram", "Test Telegram"),
    ("Роль", "Role"),
    ("Создан", "Created"),
    ("Действия", "Actions"),
    ("Добавить пользователя", "Add user"),
    ("Название группы", "Group name"),
    ("Название", "Name"),
    ("Regex паттерн", "Regex pattern"),
    ("Тип", "Type"),
    ("Порядок", "Order"),
    ("Regex (напр. ^012-)", "Regex (e.g. ^012-)"),
    ("Добавить группу", "Add group"),
    ("Политика: {lockout_state}. Настраивается во вкладке «⚙ Настройки». Список сбрасывается при перезапуске службы.",
     "Policy: {lockout_state}. Configured on the “⚙ Settings” tab. The list is reset when the service restarts."),
    ("Последний логин", "Last username"),
    ("Неудач", "Failures"),
    ("С момента", "Since"),
    ("Действие", "Action"),
    ("Снять все блокировки и сбросить счётчики?", "Clear all lockouts and reset counters?"),
    // обновления
    ("RestLex — Обновления", "RestLex — Updates"),
    ("сброс запрошен — применится на ближайшем цикле", "reset requested — applies on the next cycle"),
    ("Сбросить", "Reset"),
    ("до {until}", "until {until}"),
    ("Нет хостов с заблокированными обновлениями", "No hosts with blocked updates"),
    ("Заблокировано", "Blocked until"),
    ("Время", "Time"),
    ("Компонент", "Component"),
    ("Было", "From"),
    ("Стало", "To"),
    ("Очистить всю историю обновлений?", "Clear the entire update history?"),
    ("Очистить всё", "Clear all"),
    ("Фильтры истории обновлений", "Update history filters"),
    ("История обновлений", "Update history"),
    ("Успешные", "Successful"),
    ("Заблокированные обновления", "Blocked updates"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn hdrs(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs { h.append(k.clone(), HeaderValue::from_str(v).unwrap()); }
        h
    }

    #[test]
    fn lang_from_accept_language_and_cookie() {
        // Accept-Language: ru/be/uk/kk → RU, остальное → EN, учитывается q.
        assert_eq!(from_accept_language("ru-RU,ru;q=0.9,en;q=0.8"), Lang::Ru);
        assert_eq!(from_accept_language("be"), Lang::Ru);
        assert_eq!(from_accept_language("uk-UA"), Lang::Ru);
        assert_eq!(from_accept_language("kk;q=0.7, de;q=0.5"), Lang::Ru);
        assert_eq!(from_accept_language("en-US,en;q=0.9"), Lang::En);
        assert_eq!(from_accept_language("de-DE"), Lang::En);
        assert_eq!(from_accept_language("en;q=0.3, ru;q=0.8"), Lang::Ru);
        assert_eq!(from_accept_language("ru;q=0, en"), Lang::En);
        assert_eq!(from_accept_language("*"), Lang::En);
        // заголовка нет — как раньше, русский
        assert_eq!(Lang::from_headers(&HeaderMap::new()), Lang::Ru);
        assert_eq!(Lang::from_headers(&hdrs(&[(header::ACCEPT_LANGUAGE, "fr-FR")])), Lang::En);
        // cookie важнее заголовка
        let h = hdrs(&[(header::ACCEPT_LANGUAGE, "ru-RU"), (header::COOKIE, "session=abc; lang=en")]);
        assert_eq!(Lang::from_headers(&h), Lang::En);
        let h = hdrs(&[(header::ACCEPT_LANGUAGE, "en-US"), (header::COOKIE, "lang=ru")]);
        assert_eq!(Lang::from_headers(&h), Lang::Ru);
        // мусор в cookie игнорируется
        let h = hdrs(&[(header::ACCEPT_LANGUAGE, "en-US"), (header::COOKIE, "lang=xx; xlang=ru")]);
        assert_eq!(Lang::from_headers(&h), Lang::En);
    }

    #[test]
    fn translate_respects_word_boundaries_and_fill_keeps_data() {
        assert_eq!(t(Lang::En, "Все"), "All");
        assert_eq!(t(Lang::Ru, "Все"), "Все");
        // «Все» внутри «Всего» не трогаем
        assert_eq!(translate_en("Всего"), "Всего");
        let tpl = t(Lang::En, "<b>{p}</b> Удалить ВСЮ историю бэкапа «{p}»? {{x}}");
        let out = fill(tpl, &[("p", "Все {p}")]);
        assert_eq!(out, "<b>Все {p}</b> Delete the ENTIRE history of backup “Все {p}”? {x}");
    }
}

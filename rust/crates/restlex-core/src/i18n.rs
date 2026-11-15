//! Язык текстов, которые видят люди (Telegram, отчёты на веб-сервер, ответы
//! REST API). Источник — поле `language` глобального config.json, его пишет
//! GUI (`_init_language`). `"en"` → английский; нет поля / `"ru"` / что угодно
//! ещё → русский, как было до появления поля. Логи tracing не локализуются.

/// `true`, если язык из конфига — английский.
pub fn is_en(lang: &str) -> bool {
    lang.trim().eq_ignore_ascii_case("en")
}

/// Выбрать строку по языку: `tr(en, "русский", "English")`.
/// Для `format!`-строк — две ветки `if en { format!(..) } else { format!(..) }`.
pub fn tr<'a>(en: bool, ru: &'a str, en_text: &'a str) -> &'a str {
    if en { en_text } else { ru }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_en_switches_language() {
        assert!(is_en("en"));
        assert!(is_en(" EN "));
        assert!(!is_en(""));
        assert!(!is_en("ru"));
        assert!(!is_en("de"));
        assert_eq!(tr(true, "да", "yes"), "yes");
        assert_eq!(tr(false, "да", "yes"), "да");
    }

    #[test]
    fn old_config_without_language_is_russian() {
        let g: crate::Globals = serde_json::from_str(r#"{"tg_chat":"1"}"#).unwrap();
        assert!(!g.en());
        let g: crate::Globals = serde_json::from_str(r#"{"language":"en"}"#).unwrap();
        assert!(g.en());
        // Поле не уходит в extra и переживает запись агентом.
        assert!(!g.extra.contains_key("language"));
        assert_eq!(serde_json::to_value(&g).unwrap()["language"], "en");
        // Пустое значение не пишем — GUI сам выберет язык при первом запуске.
        let empty = serde_json::to_value(crate::Globals::default()).unwrap();
        assert!(empty.get("language").is_none());
    }
}

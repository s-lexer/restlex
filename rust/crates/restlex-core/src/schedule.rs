use chrono::{
    DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, NaiveTime, TimeZone, Timelike,
    Weekday,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Frequency {
    Daily,
    Weekly,
    Hourly,
    OnLogon,
    Disabled,
}

impl Default for Frequency {
    fn default() -> Self {
        Self::Disabled
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    #[serde(default)]
    pub freq: Frequency,
    /// HH:MM — primary time slot, backward compat
    #[serde(default = "default_time")]
    pub time: String,
    /// Additional time slots: ["02:00", "10:00"]. Merged with `time` for scheduling.
    #[serde(default)]
    pub times: Vec<String>,
    /// "MON,TUE,..."
    #[serde(default = "default_days")]
    pub days: String,
    #[serde(default = "default_interval")]
    pub interval: u32,
    #[serde(default)]
    pub forget: bool,
    #[serde(default)]
    pub notify_tg: bool,
    /// Пауза/возобновление профиля БЕЗ потери частоты/времени. `false` = профиль
    /// не запускается по расписанию (ручной `/backup` по-прежнему работает).
    /// `#[serde(default = "default_true")]` — старые profiles.json без поля читаются
    /// как «не на паузе», чтобы миграция не приостановила существующие бэкапы.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Per-task overlay parameters, override profile/globals when present.
    /// Keys: no_scan, use_vss, pre_unlock, force_unlock_orphan, stale_grace_minutes,
    /// pre_repair_index, exclude (extra), extra_args (free CLI args).
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
    /// Ключи расписания, которых агент не знает (GUI пишет сюда `run_as`).
    /// Без этого update_last_run вырезал бы их при обратной записи, и GUI
    /// пересоздавал задачу под SYSTEM вместо пользователя.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// Ручной Default воспроизводит ровно прежнее поведение derive(Default)
// (пустые time/days, interval 0 — freq=Disabled делает их несущественными),
// добавляя только enabled=true, чтобы профиль по умолчанию НЕ был на паузе.
impl Default for Schedule {
    fn default() -> Self {
        Self {
            freq: Frequency::default(),
            time: String::new(),
            times: Vec::new(),
            days: String::new(),
            interval: 0,
            forget: false,
            notify_tg: false,
            enabled: true,
            params: serde_json::Map::new(),
            extra: serde_json::Map::new(),
        }
    }
}

fn default_time() -> String {
    "02:00".into()
}
fn default_days() -> String {
    "MON".into()
}
fn default_interval() -> u32 {
    4
}
fn default_true() -> bool {
    true
}

impl Schedule {
    pub fn is_enabled(&self) -> bool {
        !matches!(self.freq, Frequency::Disabled)
    }

    /// All configured time slots (union of `time` + `times`, deduped).
    fn effective_times(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if !self.time.is_empty() {
            v.push(self.time.clone());
        }
        for t in &self.times {
            if !t.is_empty() && !v.contains(t) {
                v.push(t.clone());
            }
        }
        if v.is_empty() {
            v.push("02:00".to_string());
        }
        v
    }

    /// Returns whether this schedule should fire right now given the last fire time.
    pub fn should_fire(&self, now: DateTime<Local>, last: Option<DateTime<Local>>) -> bool {
        // last_run из будущего (скачок часов по NTP/BIOS) навсегда блокировал бы
        // расписание: `now - l >= interval` и `l < target` вечно false.
        let last = match last {
            Some(l) if l > now => {
                tracing::warn!("last_run {l} is in the future (now {now}); treating as never run");
                None
            }
            other => other,
        };
        match self.freq {
            Frequency::Disabled | Frequency::OnLogon => false,
            Frequency::Daily => self
                .effective_times()
                .iter()
                .any(|t| self.slot_fires(now, last, t)),
            Frequency::Weekly => {
                if !self
                    .days
                    .split(',')
                    .any(|d| d.trim().eq_ignore_ascii_case(weekday_code(now.weekday())))
                {
                    return false;
                }
                self.effective_times()
                    .iter()
                    .any(|t| self.slot_fires(now, last, t))
            }
            Frequency::Hourly => {
                let interval = Duration::hours(self.interval.max(1) as i64);
                last.map(|l| now - l >= interval).unwrap_or(true)
            }
        }
    }

    /// Check if a single time slot fires: now >= slot_time AND last < slot_time.
    fn slot_fires(&self, now: DateTime<Local>, last: Option<DateTime<Local>>, time_str: &str) -> bool {
        match self.target_at(now, time_str) {
            Some(target) => now >= target && last.map(|l| l < target).unwrap_or(true),
            None => false,
        }
    }

    fn target_at(&self, now: DateTime<Local>, time_str: &str) -> Option<DateTime<Local>> {
        let t = NaiveTime::parse_from_str(time_str, "%H:%M").ok()?;
        // В день перехода на летнее/зимнее время `.single()` даёт None и слот
        // молча пропускался: неоднозначное время берём раннее, несуществующее
        // (дыра весной) сдвигаем на +1 час.
        match Local.with_ymd_and_hms(now.year(), now.month(), now.day(), t.hour(), t.minute(), 0) {
            LocalResult::Single(dt) => Some(dt),
            LocalResult::Ambiguous(earliest, _) => Some(earliest),
            LocalResult::None => {
                let naive = NaiveDate::from_ymd_opt(now.year(), now.month(), now.day())?
                    .and_hms_opt(t.hour(), t.minute(), 0)?
                    + Duration::hours(1);
                Local.from_local_datetime(&naive).earliest()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Frequency, Schedule};

    #[test]
    fn legacy_schedule_without_enabled_is_not_paused() {
        // Старый profiles.json без поля `enabled` не должен приостановить бэкап.
        let s: Schedule = serde_json::from_value(serde_json::json!({
            "freq": "daily", "time": "03:00"
        }))
        .expect("deserialize legacy schedule");
        assert!(s.enabled, "missing `enabled` must default to true (not paused)");
    }

    #[test]
    fn explicit_pause_round_trips() {
        let s: Schedule = serde_json::from_value(serde_json::json!({
            "freq": "daily", "time": "03:00", "enabled": false
        }))
        .expect("deserialize paused schedule");
        assert!(!s.enabled);
        // И сериализуется обратно с enabled=false.
        let v = serde_json::to_value(&s).expect("serialize");
        assert_eq!(v.get("enabled").and_then(|x| x.as_bool()), Some(false));
    }

    #[test]
    fn unknown_sched_key_survives_round_trip() {
        // GUI пишет в sched ключ run_as; агент обязан вернуть его на диск.
        let s: Schedule = serde_json::from_value(serde_json::json!({
            "freq": "daily", "time": "03:00", "run_as": "DOMAIN\\user"
        }))
        .expect("deserialize schedule with unknown key");
        assert_eq!(
            s.extra.get("run_as").and_then(|v| v.as_str()),
            Some("DOMAIN\\user")
        );
        let v = serde_json::to_value(&s).expect("serialize");
        assert_eq!(
            v.get("run_as").and_then(|x| x.as_str()),
            Some("DOMAIN\\user"),
            "run_as must survive deserialize -> serialize"
        );
    }

    #[test]
    fn future_last_run_does_not_block_schedule() {
        use chrono::{Duration, Local};
        let now = Local::now();
        let future = now + Duration::days(400); // скачок часов вперёд

        let daily: Schedule = serde_json::from_value(serde_json::json!({
            "freq": "daily", "time": "00:00"
        }))
        .expect("daily");
        assert!(
            daily.should_fire(now, Some(future)),
            "daily slot must fire despite a future last_run"
        );

        let hourly: Schedule = serde_json::from_value(serde_json::json!({
            "freq": "hourly", "interval": 1
        }))
        .expect("hourly");
        assert!(
            hourly.should_fire(now, Some(future)),
            "hourly must fire despite a future last_run"
        );
    }

    #[test]
    fn default_schedule_is_enabled() {
        let s = Schedule::default();
        assert!(s.enabled);
        assert_eq!(s.freq, Frequency::Disabled);
    }
}

fn weekday_code(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "MON",
        Weekday::Tue => "TUE",
        Weekday::Wed => "WED",
        Weekday::Thu => "THU",
        Weekday::Fri => "FRI",
        Weekday::Sat => "SAT",
        Weekday::Sun => "SUN",
    }
}

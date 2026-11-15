use crate::{
    error::{Error, Result},
    schedule::Schedule,
};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use std::sync::RwLock;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::{collections::HashMap, fs, path::PathBuf, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoType {
    Local,
    Sftp,
    S3,
    B2,
    Azure,
    Gs,
    Rest,
}

impl Default for RepoType {
    fn default() -> Self {
        Self::Local
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Retention {
    #[serde(default, rename = "keep-last", alias = "keep_last")]
    pub keep_last: u32,
    #[serde(default, rename = "keep-hourly", alias = "keep_hourly")]
    pub keep_hourly: u32,
    #[serde(default, rename = "keep-daily", alias = "keep_daily")]
    pub keep_daily: u32,
    #[serde(default, rename = "keep-weekly", alias = "keep_weekly")]
    pub keep_weekly: u32,
    #[serde(default, rename = "keep-monthly", alias = "keep_monthly")]
    pub keep_monthly: u32,
    #[serde(default, rename = "keep-yearly", alias = "keep_yearly")]
    pub keep_yearly: u32,
    /// Ключи retention, которых агент не знает (пишет GUI/REST API) — без этого
    /// любая запись из агента их стирала бы.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Profile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub repo_type: RepoType,
    #[serde(default)]
    pub repo_path: String,
    #[serde(default)]
    pub repo_password: String,
    #[serde(default = "default_vss")]
    pub use_vss: bool,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub sched: Schedule,
    #[serde(default, deserialize_with = "deserialize_datetime_opt")]
    pub created: Option<DateTime<Local>>,
    #[serde(default, deserialize_with = "deserialize_datetime_opt")]
    pub updated: Option<DateTime<Local>>,
    #[serde(default, deserialize_with = "deserialize_datetime_opt")]
    pub last_run: Option<DateTime<Local>>,
    #[serde(default)]
    pub last_status: Option<String>,

    // Cloud credential bag
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_vss() -> bool {
    true
}

/// Default number of attempts for background network sends (report/telegram).
pub const DEFAULT_NET_RETRY_COUNT: u32 = 60;
/// Default delay between those attempts, in seconds.
pub const DEFAULT_NET_RETRY_INTERVAL_S: u64 = 60;

fn default_vss_timeout_s() -> u64 { 300 }

fn default_net_retry_count() -> u32 {
    DEFAULT_NET_RETRY_COUNT
}
fn default_net_retry_interval_s() -> u64 {
    DEFAULT_NET_RETRY_INTERVAL_S
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Globals {
    #[serde(default, alias = "restic_path")]
    pub restic: String,
    #[serde(default)]
    pub temp: String,
    #[serde(default, alias = "password_file")]
    pub default_password_file: String,
    #[serde(default)]
    pub tg_token: String,
    #[serde(default)]
    pub tg_chat: String,
    /// Thread ID для Telegram-супергрупп (message_thread_id). Пустая строка = общий чат.
    #[serde(default)]
    pub tg_group: String,
    #[serde(default)]
    pub tg_fatal: bool,
    #[serde(default)]
    pub tg_file: bool,
    #[serde(default)]
    pub tg_prune: bool,
    #[serde(default)]
    pub report_url: String,
    #[serde(default)]
    pub report_api_key: String,
    #[serde(default)]
    pub agent_auto_update_url: String,
    #[serde(default)]
    pub update_server_url: String,
    #[serde(default)]
    pub update_api_key: String,
    #[serde(default = "default_auto_update")]
    pub auto_update: bool,
    #[serde(default = "default_update_interval")]
    pub update_check_interval_m: u64,
    /// Bearer-token для /api/v1/*. Пустая строка — API отключено.
    #[serde(default)]
    pub api_token: String,
    /// One-way тумблер шифрования секретов (repo_password, tg_token, *_api_key)
    /// DPAPI'ем на диске. Если ключа нет в config.json, Config::load защёлкивает
    /// его по факту наличия enc: в данных (не "даунгрейдим" существующее шифрование).
    /// Должен совпадать с логикой GUI (restlex.py: encrypt_secrets).
    #[serde(default)]
    pub encrypt_secrets: bool,
    /// Передавать --no-scan в restic backup (пропустить сканирование файлов перед бэкапом).
    #[serde(default)]
    pub global_no_scan: bool,
    /// Глобальная команда перед бэкапом (выполняется в scheduler и GUI).
    #[serde(default)]
    pub global_pre_backup_cmd: String,
    /// Таймаут создания теневой копии VSS, сек (restic `-o vss.timeout`).
    /// Дефолт restic — 120с, нам его мало на нагруженных серверах.
    #[serde(default = "default_vss_timeout_s")]
    pub global_vss_timeout_s: u64,
    /// Number of attempts for background network sends (report/telegram).
    #[serde(default = "default_net_retry_count")]
    pub net_retry_count: u32,
    /// Delay between background network send attempts, in seconds.
    #[serde(default = "default_net_retry_interval_s")]
    pub net_retry_interval_s: u64,
    /// Язык текстов для людей (Telegram, отчёты, ответы API): "en" | "ru".
    /// Пишет GUI; пусто/нет поля — русский (см. `crate::i18n`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,

    /// Всё, что пишет в config.json GUI и не знает агент (global_check_host_only,
    /// restore_ui_mode и т.п.). Без этого любая запись из агента стирала бы
    /// настройки GUI — писать Globals целиком было бы нельзя.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for Globals {
    fn default() -> Self {
        Self {
            restic: String::new(),
            temp: String::new(),
            default_password_file: String::new(),
            tg_token: String::new(),
            tg_chat: String::new(),
            tg_group: String::new(),
            tg_fatal: false,
            tg_file: false,
            tg_prune: false,
            report_url: String::new(),
            report_api_key: String::new(),
            agent_auto_update_url: String::new(),
            update_server_url: String::new(),
            update_api_key: String::new(),
            auto_update: default_auto_update(),
            update_check_interval_m: default_update_interval(),
            api_token: String::new(),
            encrypt_secrets: false,
            global_no_scan: false,
            global_pre_backup_cmd: String::new(),
            global_vss_timeout_s: default_vss_timeout_s(),
            net_retry_count: DEFAULT_NET_RETRY_COUNT,
            net_retry_interval_s: DEFAULT_NET_RETRY_INTERVAL_S,
            language: String::new(),
            extra: HashMap::new(),
        }
    }
}

impl Globals {
    /// Тексты для людей — по-английски (`language == "en"`).
    pub fn en(&self) -> bool {
        crate::i18n::is_en(&self.language)
    }
}

fn default_auto_update() -> bool { true }
fn default_update_interval() -> u64 { 30 }

#[derive(Clone)]
pub struct Config {
    inner: Arc<RwLock<Inner>>,
}

struct Inner {
    globals: Globals,
    profiles: HashMap<String, Profile>,
    cfg_path: PathBuf,
    pr_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct ProfileCompat {
    #[serde(default)]
    name: String,
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default, alias = "exclude_patterns")]
    exclude: Vec<String>,
    #[serde(default)]
    repo_type: RepoType,
    #[serde(default)]
    repo_path: String,
    #[serde(default)]
    repo_password: String,
    #[serde(default = "default_vss")]
    use_vss: bool,
    #[serde(default, alias = "forget_policy")]
    retention: Retention,
    #[serde(default)]
    sched: Schedule,
    #[serde(default, deserialize_with = "deserialize_datetime_opt")]
    created: Option<DateTime<Local>>,
    #[serde(default, deserialize_with = "deserialize_datetime_opt")]
    updated: Option<DateTime<Local>>,
    #[serde(default, deserialize_with = "deserialize_datetime_opt")]
    last_run: Option<DateTime<Local>>,
    #[serde(default)]
    last_status: Option<String>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

impl From<ProfileCompat> for Profile {
    fn from(mut value: ProfileCompat) -> Self {
        normalize_legacy_extra_keys(&mut value.extra);
        Self {
            name: value.name,
            sources: value.sources,
            exclude: value.exclude,
            repo_type: value.repo_type,
            repo_path: value.repo_path,
            repo_password: value.repo_password,
            use_vss: value.use_vss,
            retention: value.retention,
            sched: value.sched,
            created: value.created,
            updated: value.updated,
            last_run: value.last_run,
            last_status: value.last_status,
            extra: value.extra,
        }
    }
}

impl<'de> Deserialize<'de> for Profile {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        ProfileCompat::deserialize(deserializer).map(Into::into)
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let base = crate::program_data();
        fs::create_dir_all(&base)?;
        fs::create_dir_all(crate::logs_dir())?;
        let cfg_path = crate::config_path();
        let pr_path = crate::profiles_path();
        let globals = read_globals_resolved(&cfg_path, &Globals::default());
        let profiles = read_profiles_safe(&pr_path, &HashMap::new());
        Ok(Self {
            inner: Arc::new(RwLock::new(Inner {
                globals,
                profiles,
                cfg_path,
                pr_path,
            })),
        })
    }

    pub fn reload(&self) -> Result<()> {
        let mut g = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let gl_fallback = g.globals.clone();
        g.globals = read_globals_resolved(&g.cfg_path, &gl_fallback);
        let fallback = g.profiles.clone();
        g.profiles = read_profiles_safe(&g.pr_path, &fallback);
        Ok(())
    }

    pub fn globals(&self) -> Globals {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).globals.clone()
    }

    pub fn set_globals(&self, g: Globals) -> Result<()> {
        let mut w = self.inner.write().unwrap_or_else(|e| e.into_inner());
        // ponytail: file-lock берётся под RwLock-гардом — блокирующий cross-process
        // лок держит async-поток; чинить вместе с переходом на try_lock+spawn_blocking
        // (меняет сигнатуры пол-крейта, отдельная задача).
        let _lk = config_file_lock();
        let enc = g.encrypt_secrets;
        // Сначала диск, потом память: иначе провалившийся write_atomic оставляет
        // in-memory и файл рассогласованными до перезапуска службы.
        write_atomic(&w.cfg_path, &encrypt_globals(&g, enc))?;
        w.globals = g;
        Ok(())
    }

    /// Наложить частичный патч на глобальные настройки: перечитываем свежий
    /// диск под локом, накладываем только присланные ключи и пишем обратно.
    /// Так параллельная правка из GUI не теряется, а ключи, которых агент не
    /// знает, переживают запись через `Globals::extra`.
    pub fn patch_globals(
        &self,
        patch: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Globals> {
        let mut w = self.inner.write().unwrap_or_else(|e| e.into_inner());
        // ponytail: см. set_globals — блокирующий file-lock под RwLock-гардом.
        // ponytail: patch_globals не перешифровывает profiles.json при смене
        // encrypt_secrets; чинить отдельной задачей (нужен проход по профилям).
        let _lk = config_file_lock();
        // Читаем диск как СЫРОЙ JSON-объект, а не как Globals. Типизированный
        // разбор здесь недопустим: одно поле с неожиданным типом (например от
        // более новой версии GUI) — это ошибка serde, и файл уезжал бы в
        // карантин, а на его место писались ДЕФОЛТЫ (терялись tg_token,
        // report_url, restic, интервалы). Карантин — только для реально
        // неразбираемого JSON, иначе настройки из Recondeck/REST нельзя было бы
        // сохранить НИКОГДА, пока файл не поправят руками.
        // IO-ошибка (sharing violation от антивируса) — ретраим, потом Err.
        let mut v: Value = match read_config_text_retry(&w.cfg_path) {
            Ok(None) => serde_json::to_value(&w.globals)?,
            Ok(Some(txt)) => match serde_json::from_str::<Value>(&txt) {
                Ok(v) if v.is_object() => v,
                Ok(_) => {
                    quarantine_bad_config(&w.cfg_path, "config.json не JSON-объект");
                    serde_json::to_value(&w.globals)?
                }
                Err(e) => {
                    quarantine_bad_config(&w.cfg_path, &e.to_string());
                    serde_json::to_value(&w.globals)?
                }
            },
            Err(e) => return Err(Error::from(e)),
        };
        if let Some(obj) = v.as_object_mut() {
            for (k, val) in patch {
                obj.insert(k.clone(), val.clone());
            }
        }
        // Типизируем уже ПОСЛЕ наложения патча. Если не типизируется — Err и
        // файл остаётся нетронутым (лучше не сохранить патч, чем потерять
        // настройки).
        let mut merged: Globals = serde_json::from_value(v)
            .map_err(|e| Error::BadPatch(e.to_string()))?;
        // Как в read_globals_resolved: на диске секреты лежат как "enc:...",
        // а в памяти обязаны быть расшифрованными.
        decrypt_globals(&mut merged);
        let enc = merged.encrypt_secrets;
        write_atomic(&w.cfg_path, &encrypt_globals(&merged, enc))?;
        w.globals = merged.clone();
        Ok(merged)
    }

    pub fn profiles(&self) -> HashMap<String, Profile> {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).profiles.clone()
    }

    pub fn profile(&self, pid: &str) -> Option<Profile> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .profiles
            .get(pid)
            .cloned()
    }

    pub fn save_profile(&self, pid: &str, mut p: Profile) -> Result<()> {
        let now = Local::now();
        let mut w = self.inner.write().unwrap_or_else(|e| e.into_inner());
        // ponytail: см. set_globals — блокирующий file-lock под RwLock-гардом.
        let _lk = config_file_lock();
        // Re-read from disk so a concurrent writer (the agent's update_last_run,
        // or another GUI instance) does not get clobbered by our stale in-memory
        // map. We only mutate the single profile we own here.
        let (mut on_disk, broken) = read_profiles_with_raw(&w.pr_path, &w.profiles);
        let created = on_disk
            .get(pid)
            .or_else(|| w.profiles.get(pid))
            .and_then(|old| old.created)
            .or(Some(now));
        p.created = created;
        p.updated = Some(now);
        on_disk.insert(pid.into(), p);
        let enc = w.globals.encrypt_secrets;
        write_profiles(&w.pr_path, &on_disk, &broken, enc)?;
        w.profiles = on_disk;
        Ok(())
    }

    pub fn del_profile(&self, pid: &str) -> Result<()> {
        let mut w = self.inner.write().unwrap_or_else(|e| e.into_inner());
        // ponytail: см. set_globals — блокирующий file-lock под RwLock-гардом.
        let _lk = config_file_lock();
        let (mut on_disk, mut broken) = read_profiles_with_raw(&w.pr_path, &w.profiles);
        on_disk.remove(pid);
        broken.remove(pid);
        let enc = w.globals.encrypt_secrets;
        write_profiles(&w.pr_path, &on_disk, &broken, enc)?;
        w.profiles = on_disk;
        let lf = crate::logs_dir().join(format!("{}.log", pid));
        let _ = fs::remove_file(lf);
        Ok(())
    }

    pub fn update_last_run(&self, pid: &str, status: &str) -> Result<()> {
        let mut w = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let _lk = config_file_lock();
        // Merge onto the freshest on-disk copy so we never drop credentials or
        // other profiles that the GUI saved while a backup was running.
        let (mut on_disk, broken) = read_profiles_with_raw(&w.pr_path, &w.profiles);
        if !on_disk.contains_key(pid) {
            if let Some(p) = w.profiles.get(pid) {
                on_disk.insert(pid.into(), p.clone());
            }
        }
        if let Some(p) = on_disk.get_mut(pid) {
            p.last_run = Some(Local::now());
            p.last_status = Some(status.to_string());
        }
        let enc = w.globals.encrypt_secrets;
        write_profiles(&w.pr_path, &on_disk, &broken, enc)?;
        w.profiles = on_disk;
        Ok(())
    }
}

/// Read the profiles map from disk, falling back to the in-memory copy when the
/// file is missing, empty, or fails to parse. This prevents a transient/partial
/// read from collapsing into an empty map that would then wipe every profile.
fn read_profiles_safe(
    path: &PathBuf,
    fallback: &HashMap<String, Profile>,
) -> HashMap<String, Profile> {
    read_profiles_with_raw(path, fallback).0
}

/// Как [`read_profiles_safe`], но дополнительно возвращает сырые JSON-значения
/// профилей, которые не удалось разобрать (кривая дата в `last_run`, неизвестный
/// `repo_type`). Раньше одна такая запись роняла разбор ВСЕЙ мапы, и первая же
/// запись на диск уничтожала остальные профили. Теперь битые пропускаются с
/// ошибкой в лог, а их сырой Value переносится в новую версию файла.
fn read_profiles_with_raw(
    path: &PathBuf,
    fallback: &HashMap<String, Profile>,
) -> (HashMap<String, Profile>, serde_json::Map<String, Value>) {
    let s = match fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return (fallback.clone(), serde_json::Map::new()),
    };
    let raw: serde_json::Map<String, Value> = match serde_json::from_str(&s) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("profiles.json is not a JSON object ({e}); keeping in-memory profiles");
            return (fallback.clone(), serde_json::Map::new());
        }
    };
    let mut good: HashMap<String, Profile> = HashMap::new();
    let mut broken = serde_json::Map::new();
    for (pid, v) in raw {
        match serde_json::from_value::<Profile>(v.clone()) {
            Ok(p) => {
                good.insert(pid, p);
            }
            Err(e) => {
                tracing::error!("profile '{pid}' is malformed ({e}); kept as-is, skipped in agent");
                broken.insert(pid, v);
            }
        }
    }
    decrypt_profiles(&mut good);
    (good, broken)
}

/// Сериализовать мапу профилей вместе с нетронутыми «битыми» записями и записать
/// атомарно. Битый профиль не должен исчезать с диска из-за нашей записи.
fn write_profiles(
    path: &PathBuf,
    m: &HashMap<String, Profile>,
    broken: &serde_json::Map<String, Value>,
    enc: bool,
) -> Result<()> {
    let mut v = serde_json::to_value(encrypt_profiles(m, enc))?;
    if let Some(obj) = v.as_object_mut() {
        for (pid, raw) in broken {
            obj.entry(pid.clone()).or_insert_with(|| raw.clone());
        }
    }
    write_atomic(path, &v)
}

/// Decrypt the `enc:`-prefixed secrets in a freshly-read profiles map so every
/// in-memory consumer (restic runner, REST API, mount) sees plaintext. The
/// on-disk form stays encrypted — see [`encrypt_profiles`] on the write path.
fn decrypt_profiles(m: &mut HashMap<String, Profile>) {
    for p in m.values_mut() {
        p.repo_password = crate::crypto::decrypt_secret(&p.repo_password);
    }
}

/// Re-encrypt profile secrets right before serialising to disk — only when the
/// one-way `encrypt_secrets` toggle is on. Mirrors the Python GUI's `_sv`. When
/// off, returns the plaintext clone unchanged (no downgrade of any value that is
/// still `enc:` — those were decrypted on load only if the toggle/latch was on).
/// Returns a clone so the in-memory cache keeps the plaintext value.
fn encrypt_profiles(m: &HashMap<String, Profile>, enabled: bool) -> HashMap<String, Profile> {
    let mut out = m.clone();
    if enabled {
        for p in out.values_mut() {
            p.repo_password = crate::crypto::encrypt_secret(&p.repo_password);
        }
    }
    out
}

/// Decrypt the `enc:`-prefixed global secrets after reading config.json.
/// Список ключей обязан совпадать с `encrypt_globals` и со списком секретов в
/// GUI (`restlex.py`, `SECRET_KEYS`) — иначе агент сравнивает Bearer-токен с
/// base64-блобом и отдаёт 401 на всё.
fn decrypt_globals(g: &mut Globals) {
    g.tg_token = crate::crypto::decrypt_secret(&g.tg_token);
    g.report_api_key = crate::crypto::decrypt_secret(&g.report_api_key);
    g.update_api_key = crate::crypto::decrypt_secret(&g.update_api_key);
    g.api_token = crate::crypto::decrypt_secret(&g.api_token);
}

/// Re-encrypt global secrets before writing config.json — only when the
/// `encrypt_secrets` toggle is on. Returns a clone so the in-memory copy stays
/// plaintext for consumers.
fn encrypt_globals(g: &Globals, enabled: bool) -> Globals {
    let mut out = g.clone();
    if enabled {
        out.tg_token = crate::crypto::encrypt_secret(&out.tg_token);
        out.report_api_key = crate::crypto::encrypt_secret(&out.report_api_key);
        out.update_api_key = crate::crypto::encrypt_secret(&out.update_api_key);
        out.api_token = crate::crypto::encrypt_secret(&out.api_token);
    }
    out
}

/// Прочитать config.json с ретраями на транзиентную IO-ошибку (sharing violation
/// от антивируса/индексатора). `Ok(None)` — файла нет или он пуст.
fn read_config_text_retry(path: &PathBuf) -> std::io::Result<Option<String>> {
    let mut last: Option<std::io::Error> = None;
    for attempt in 0..3 {
        match fs::read_to_string(path) {
            Ok(t) if t.trim().is_empty() => return Ok(None),
            Ok(t) => return Ok(Some(t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                tracing::warn!("config.json read failed (attempt {}): {e}", attempt + 1);
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("config.json read failed")))
}

/// Битый config.json уносим в `config.json.bad-<timestamp>` (не затирая
/// предыдущий) и даём вызывающему записать новый — иначе одна кривая правка
/// файла навсегда блокировала бы сохранение настроек из Recondeck/REST.
fn quarantine_bad_config(path: &PathBuf, why: &str) {
    // Миллисекунды в штампе обязательны: два карантина в одну секунду
    // переименовывались в ОДНО имя, а rename на Windows перезаписывает —
    // предыдущая копия битого конфига терялась.
    let stamp = Local::now().format("%Y%m%d-%H%M%S%.3f");
    let bad = path.with_file_name(format!(
        "{}.bad-{stamp}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("config.json")
    ));
    match fs::rename(path, &bad) {
        Ok(()) => tracing::error!("config.json не разобран ({why}); перемещён в {}", bad.display()),
        Err(e) => tracing::error!("config.json не разобран ({why}); переименовать не вышло: {e}"),
    }
}

/// Read and decrypt globals from config.json. `encrypt_secrets` follows the GUI:
/// default off (serde default `false`) until the user explicitly enables it. We
/// do NOT infer it from existing `enc:` data — the toggle is user-driven on both
/// sides. `decrypt_secret` still transparently reads any `enc:` left on disk.
/// При ошибке чтения/разбора возвращаем `fallback` (текущие in-memory globals),
/// как это делает `read_profiles_safe`: временный sharing violation не должен
/// молча выключать отчёты/Telegram и сбрасывать `encrypt_secrets` в false.
fn read_globals_resolved(cfg_path: &PathBuf, fallback: &Globals) -> Globals {
    let cfg_text = match fs::read_to_string(cfg_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            tracing::error!("config.json read failed ({e}); keeping in-memory globals");
            return fallback.clone();
        }
    };
    if cfg_text.trim().is_empty() {
        return Globals::default();
    }
    let mut globals: Globals = match serde_json::from_str(&cfg_text) {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("config.json parse failed ({e}); keeping in-memory globals");
            return fallback.clone();
        }
    };
    decrypt_globals(&mut globals);
    globals
}

/// Acquire an exclusive cross-process lock on the config directory for the
/// duration of a read-modify-write. The GUI (user session) and the agent
/// (LocalSystem, session 0) both mutate the same JSON files; a file lock works
/// across sessions without the privilege a `Global\` mutex would need. The
/// returned guard releases the lock when dropped. Best-effort: `None` means we
/// proceed unlocked rather than block config edits entirely.
pub fn config_file_lock() -> Option<std::fs::File> {
    use fs2::FileExt;
    let path = crate::program_data().join(".config.lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .ok()?;
    f.lock_exclusive().ok()?;
    Some(f)
}

fn deserialize_datetime_opt<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<DateTime<Local>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::String(s) => {
            if s.trim().is_empty() {
                Ok(None)
            } else {
                parse_datetime_opt(&s)
                    .map(Some)
                    .ok_or_else(|| serde::de::Error::custom(format!("invalid datetime: {s}")))
            }
        }
        Value::Null => Ok(None),
        other => Err(serde::de::Error::custom(format!(
            "invalid datetime value: {other}"
        ))),
    }
}

fn parse_datetime_opt(raw: &str) -> Option<DateTime<Local>> {
    if raw.trim().is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Local));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f") {
        return Local.from_local_datetime(&dt).single();
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f") {
        return Local.from_local_datetime(&dt).single();
    }
    None
}

fn normalize_legacy_extra_keys(extra: &mut HashMap<String, Value>) {
    const LEGACY_ALIASES: &[(&str, &str)] = &[
        ("sftp_u", "cred_sftp_user"),
        ("sftp_p", "cred_sftp_pass"),
        ("s3_k", "cred_s3_key"),
        ("s3_s", "cred_s3_secret"),
        ("b2_i", "cred_b2_account"),
        ("b2_k", "cred_b2_key"),
        ("az_a", "cred_az_account"),
        ("az_k", "cred_az_key"),
        ("gs_p", "cred_gs_project"),
        ("gs_j", "cred_gs_json"),
        ("rest_host", "cred_rest_host"),
        ("rest_path", "cred_rest_path"),
        ("rest_user", "cred_rest_user"),
        ("rest_pass", "cred_rest_pass"),
    ];

    for (legacy, current) in LEGACY_ALIASES {
        if !extra.contains_key(*current) {
            if let Some(value) = extra.get(*legacy).cloned() {
                extra.insert((*current).to_string(), value);
            }
        }
    }
}

pub fn write_atomic<T: Serialize>(p: &PathBuf, data: &T) -> Result<()> {
    let s = serde_json::to_string_pretty(data)?;
    // Per-process temp name: the GUI and the agent share this directory, so a
    // fixed "*.tmp" name lets two processes corrupt each other's staging file.
    let tmp = p.with_extension(format!("tmp.{}", std::process::id()));
    // Flush staged bytes to disk before the rename so a crash can't leave the
    // destination pointing at a half-written file. sync_all обязан вызываться на
    // ПИШУЩЕМ хэндле: на read-only хэндле FlushFileBuffers под Windows падает.
    {
        use std::io::Write;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(s.as_bytes())?;
        f.sync_all()?;
    }
    // std::fs::rename replaces the destination atomically (on Windows it maps to
    // MoveFileEx with REPLACE_EXISTING). Never remove the destination first —
    // that opens a window where the config file does not exist and a concurrent
    // read collapses to an empty/default config. Retry transient Windows sharing
    // violations (watcher/AV briefly holding the handle).
    let mut last_err = None;
    for attempt in 0..5u32 {
        match fs::rename(&tmp, p) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(
                    20 * u64::from(attempt + 1),
                ));
            }
        }
    }
    let _ = fs::remove_file(&tmp);
    Err(last_err
        .map(Error::from)
        .unwrap_or_else(|| Error::Other("atomic write failed".into())))
}

#[cfg(test)]
mod tests {
    use super::{read_profiles_with_raw, write_profiles, Globals, Profile, Value};
    #[allow(unused_imports)]
    use crate::crypto::ENC_PREFIX;

    #[test]
    fn broken_profile_does_not_wipe_the_others() {
        // Одна битая запись (невалидная дата + неизвестный repo_type) раньше
        // роняла разбор всей мапы, и следующая запись стирала остальные профили.
        let path = std::env::temp_dir()
            .join(format!("restlex-test-broken-{}.json", std::process::id()));
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "good": { "name": "good", "repo_path": "D:\\repo" },
                "bad":  { "name": "bad", "repo_type": "quantum", "last_run": "not-a-date" }
            }))
            .unwrap(),
        )
        .unwrap();

        let (mut good, broken) = read_profiles_with_raw(&path, &std::collections::HashMap::new());
        assert_eq!(good.len(), 1, "valid profile must survive");
        assert!(good.contains_key("good"));
        assert!(broken.contains_key("bad"), "broken profile kept as raw JSON");

        // Имитируем update_last_run: добавляем ещё один профиль и пишем обратно.
        good.insert("fresh".into(), Profile::default());
        write_profiles(&path, &good, &broken, false).unwrap();

        let back: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(back.contains_key("good"), "existing profile must not be lost");
        assert!(back.contains_key("fresh"));
        assert!(
            back.contains_key("bad"),
            "broken profile must stay on disk, not be silently deleted"
        );
        assert_eq!(
            back["bad"].get("repo_type").and_then(|v| v.as_str()),
            Some("quantum"),
            "broken profile must be written back byte-for-byte"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// GUI держит `api_token` в списке секретов и при `encrypt_secrets=true`
    /// пишет его как `enc:…`. Если `decrypt_globals` его пропустит, агент будет
    /// сравнивать Bearer с base64-блобом и отдавать 401 на всё.
    #[test]
    fn api_token_survives_encrypt_decrypt_roundtrip() {
        let mut g = Globals {
            api_token: "s3cret-token".into(),
            tg_token: "tg".into(),
            report_api_key: "rk".into(),
            update_api_key: "uk".into(),
            encrypt_secrets: true,
            ..Default::default()
        };
        let mut on_disk = super::encrypt_globals(&g, true);
        // На Windows DPAPI даёт enc:-форму; на других ОС encrypt_secret — no-op.
        #[cfg(windows)]
        assert!(
            on_disk.api_token.starts_with(crate::crypto::ENC_PREFIX),
            "api_token обязан шифроваться вместе с остальными секретами"
        );
        super::decrypt_globals(&mut on_disk);
        assert_eq!(on_disk.api_token, "s3cret-token");
        assert_eq!(on_disk.tg_token, "tg");
        assert_eq!(on_disk.report_api_key, "rk");
        assert_eq!(on_disk.update_api_key, "uk");
        // и обратная сторона: расшифровка не портит плейнтекст
        super::decrypt_globals(&mut g);
        assert_eq!(g.api_token, "s3cret-token");
    }

    #[test]
    fn globals_support_legacy_restic_keys() {
        let globals: Globals = serde_json::from_value(serde_json::json!({
            "restic_path": "restic.exe",
            "password_file": "C:\\secret.txt",
            "temp": "C:\\Temp"
        }))
        .expect("deserialize globals");

        assert_eq!(globals.restic, "restic.exe");
        assert_eq!(globals.default_password_file, "C:\\secret.txt");
        assert_eq!(globals.temp, "C:\\Temp");
    }

    #[test]
    fn profile_supports_legacy_profile_shape() {
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "name": "legacy",
            "sources": ["C:\\Data"],
            "exclude_patterns": ["*.tmp"],
            "forget_policy": { "keep_daily": 7, "keep-last": 2 },
            "repo_path": "D:\\repo",
            "repo_password": "pw",
            "s3_k": "key",
            "rest_host": "127.0.0.1:8000",
            "created": "2026-04-19T10:11:12"
        }))
        .expect("deserialize profile");

        assert_eq!(profile.exclude, vec!["*.tmp".to_string()]);
        assert_eq!(profile.retention.keep_daily, 7);
        assert_eq!(profile.retention.keep_last, 2);
        assert_eq!(
            profile.extra.get("cred_s3_key").and_then(|v| v.as_str()),
            Some("key")
        );
        assert_eq!(
            profile.extra.get("cred_rest_host").and_then(|v| v.as_str()),
            Some("127.0.0.1:8000")
        );
        assert!(profile.created.is_some());
    }

    /// Собирает Config поверх временных файлов, не трогая ProgramData.
    fn test_config(cfg_path: std::path::PathBuf) -> super::Config {
        super::Config {
            inner: std::sync::Arc::new(std::sync::RwLock::new(super::Inner {
                globals: Globals::default(),
                profiles: std::collections::HashMap::new(),
                pr_path: cfg_path.with_extension("profiles.json"),
                cfg_path,
            })),
        }
    }

    /// patch_globals обязан мержить патч на СЫРОЙ JSON: неизвестные агенту ключи
    /// (их пишет GUI) переживают запись, а реальные настройки не подменяются
    /// дефолтами.
    #[test]
    fn patch_globals_keeps_unknown_fields() {
        let path = std::env::temp_dir()
            .join(format!("restlex-test-patch-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"tg_token":"bot:secret","report_url":"http://srv/report",
                "restic":"D:\restic.exe","net_retry_count":7,
                "future_gui_field":{"a":1}}"#,
        )
        .unwrap();
        let cfg = test_config(path.clone());

        let mut patch = serde_json::Map::new();
        patch.insert("api_token".into(), Value::String("deadbeef".into()));
        let merged = cfg.patch_globals(&patch).expect("patch должен пройти");

        assert_eq!(merged.api_token, "deadbeef");
        assert_eq!(merged.tg_token, "bot:secret");
        assert_eq!(merged.report_url, "http://srv/report");
        assert_eq!(merged.net_retry_count, 7);

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["future_gui_field"]["a"], serde_json::json!(1));
        assert_eq!(v["tg_token"], serde_json::json!("bot:secret"));
        assert_eq!(v["api_token"], serde_json::json!("deadbeef"));
        let _ = std::fs::remove_file(&path);
    }

    /// Поле с НЕОЖИДАННЫМ типом (более новая версия GUI) не должно уводить
    /// config.json в карантин и подменяться дефолтами: патч отклоняется, файл
    /// остаётся как был.
    #[test]
    fn patch_globals_does_not_destroy_config_on_type_mismatch() {
        let path = std::env::temp_dir()
            .join(format!("restlex-test-patch-bad-{}.json", std::process::id()));
        let orig = r#"{"tg_token":"bot:secret","net_retry_count":"seven"}"#;
        std::fs::write(&path, orig).unwrap();
        let cfg = test_config(path.clone());

        let mut patch = serde_json::Map::new();
        patch.insert("api_token".into(), Value::String("deadbeef".into()));
        assert!(cfg.patch_globals(&patch).is_err(), "патч должен быть отклонён");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), orig);
        // Карантина быть не должно: JSON валиден.
        let quarantined = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .any(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.starts_with(&format!(
                    "restlex-test-patch-bad-{}.json.bad-",
                    std::process::id()
                ))
            });
        assert!(!quarantined, "валидный JSON не должен попадать в карантин");
        let _ = std::fs::remove_file(&path);
    }
}

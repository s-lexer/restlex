use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(windows)]
mod service;

const SERVICE_NAME: &str = "RestlexUpdateServer";
const SERVICE_DISPLAY: &str = "RestLex Update Server";
const SERVICE_DESC: &str = "Сервер обновлений для компонентов RestLex";
const CONFIG_PATH: &str = r"C:\ProgramData\RestLex\update-server.json";

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "restlex-update-server")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Установить и запустить службу Windows.
    ///
    /// Сохраняет конфиг в C:\ProgramData\RestLex\update-server.json и регистрирует
    /// службу RestlexUpdateServer (автозапуск, перезапуск при сбое).
    ///
    /// Структура папки (--folder):
    ///   restlex.exe          — новая версия GUI
    ///   restlex-agent.exe    — новая версия агента (опционально)
    ///   versions.json        — {"agent":"6.0.2","gui":"1.0.28"}
    ///
    /// Клиенты обращаются:
    ///   GET /api/manifest           (заголовок X-Api-Key: <api-key>)
    ///   GET /api/download?file=...  (заголовок X-Api-Key: <api-key>)
    ///
    /// Пример:
    ///   restlex-update-server.exe install --folder C:\Updates\RestLex --api-key mykey123
    ///   restlex-update-server.exe install --folder C:\Updates\RestLex --api-key mykey123 --agent-version 6.0.2 --gui-version 1.0.28
    Install {
        #[arg(long, default_value = "8901")]
        port: u16,
        /// Папка с файлами обновлений (exe + versions.json)
        #[arg(long)]
        folder: String,
        /// Секретный API-ключ (заголовок X-Api-Key). На время ротации можно
        /// перечислить несколько через запятую (`--api-key "старый,новый"`) —
        /// принимаются все. Ключ, СОДЕРЖАЩИЙ запятую, так задать нельзя:
        /// укажите `--api-key` несколько раз, тогда разбиения не происходит.
        #[arg(long, required = true)]
        api_key: Vec<String>,
        /// ОТДЕЛЬНЫЙ токен для выдачи restlex-config.json (заголовок
        /// X-Install-Key). Тот же токен должен быть зашит в инсталляторе.
        /// Не задан — restlex-config.json не отдаётся вообще.
        #[arg(long, default_value = "")]
        install_key: String,
        /// Общий сервер: программы без ключа (см. Config::public). Без флага
        /// сохраняется значение из текущего конфига — переустановка не должна
        /// молча закрыть сервер для агентов без ключа.
        #[arg(long)]
        public: bool,
        /// Версия агента для versions.json (по умолчанию 0.0.0 — обновить вручную)
        #[arg(long, default_value = "0.0.0")]
        agent_version: String,
        /// Версия GUI для versions.json (по умолчанию 0.0.0 — обновить вручную)
        #[arg(long, default_value = "0.0.0")]
        gui_version: String,
    },
    /// Удалить службу Windows (останавливает если запущена)
    Uninstall,
    /// Запустить службу (должна быть установлена через install)
    Start,
    /// Остановить службу
    Stop,
    /// Запустить в консоли без службы (читает существующий конфиг)
    Run,
    /// Внутренняя команда для SCM (не вызывать вручную)
    #[cfg(windows)]
    Service,
}

// ── Config ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Config {
    port: u16,
    folder: String,
    /// Legacy-поле: один ключ либо несколько через запятую. Разбор по запятой
    /// оставлен только для обратной совместимости чтения — секрет, содержащий
    /// запятую, из-за него разваливался на два коротких принимаемых ключа.
    #[serde(default)]
    api_key: String,
    /// Правильный способ задать несколько ключей (ротация): отдельный массив,
    /// никакого экранирования не требуется.
    #[serde(default)]
    api_keys: Vec<String>,
    /// ОТДЕЛЬНЫЙ токен для установочного конфига (`restlex-config.json`).
    /// Этот файл содержит `global`/`profiles` парка в ПЛЕЙНТЕКСТЕ (инсталлятор
    /// кладёт значения как есть, DPAPI-шифрование машинно-привязано и на
    /// сервере невозможно), поэтому он не должен отдаваться по тому же ключу,
    /// что и бинарники: ключ бинарников знает каждый агент парка.
    /// Пусто = файл не отдаётся вообще (fail-closed).
    #[serde(default)]
    install_key: String,
    /// Максимум одновременных отдач файлов. 0 = дефолт.
    #[serde(default)]
    max_concurrent_downloads: usize,
    /// Общий сервер обновлений для всех установок: manifest и подписанные
    /// программы (PUBLIC_FILES) отдаются без ключа. Целостность держит не ключ
    /// (он и так зашит в каждый exe и идёт по HTTP), а проверка Authenticode
    /// у агента. Установщики с вшитым install_key и restlex-config.json —
    /// по-прежнему только с ключами.
    #[serde(default)]
    public: bool,
}

impl Config {
    fn load() -> Result<Self> {
        let s = fs::read_to_string(CONFIG_PATH)?;
        let mut c: Config = serde_json::from_str(&s)?;
        // Понимаем оба формата: `enc:` (DPAPI) и старый плейнтекст.
        c.api_key = restlex_core::crypto::decrypt_secret_named(&c.api_key, "api_key");
        c.api_keys = c
            .api_keys
            .iter()
            .map(|k| restlex_core::crypto::decrypt_secret_named(k, "api_keys"))
            .collect();
        c.install_key = restlex_core::crypto::decrypt_secret_named(&c.install_key, "install_key");
        Ok(c)
    }

    /// Атомарная запись под кросс-процессным локом (CLAUDE.md п.6); ключи
    /// уходят на диск только в виде `enc:`.
    fn save(&self) -> Result<()> {
        let p = PathBuf::from(CONFIG_PATH);
        if let Some(d) = p.parent() {
            fs::create_dir_all(d)?;
        }
        let mut enc = self.clone();
        enc.api_key = restlex_core::crypto::encrypt_secret(&enc.api_key);
        enc.api_keys = enc
            .api_keys
            .iter()
            .map(|k| restlex_core::crypto::encrypt_secret(k))
            .collect();
        if !enc.install_key.is_empty() {
            enc.install_key = restlex_core::crypto::encrypt_secret(&enc.install_key);
        }
        let _lk = restlex_core::config::config_file_lock();
        restlex_core::config::write_atomic(&p, &enc).map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

// ── HTTP server ───────────────────────────────────────────────────────────────

/// Ключ кеша манифеста: (имя, размер, mtime-в-наносекундах) по всем файлам,
/// влияющим на манифест — exe И файлы версий. Секундной точности мало:
/// staging→move в deploy.ps1 успевает заменить бинарь тем же размером в ту же
/// секунду, и агент получал бы старый sha256 при новом файле.
type DirKey = Vec<(String, u64, u128)>;

/// Не-exe файлы, которые входят в ключ кеша (их содержимое меняет манифест).
const VERSION_FILES: &[&str] = &["versions.json", "version.txt"];

/// Не-exe файлы, которые разрешено скачивать по обычному api_key, хотя их нет
/// в манифесте. Секретов не содержат.
const DOWNLOADABLE_EXTRA: &[&str] = &["versions.json"];

/// Файлы, которые отдаются ТОЛЬКО по отдельному установочному токену
/// (`install_key`, заголовок `X-Install-Key`), а не по ключу бинарников.
/// `restlex-config.json` — это `global`/`profiles` парка в плейнтексте
/// (repo_password, tg_token, api-ключи): раздавать его по ключу, который зашит
/// в каждый агент и ездит по открытому HTTP, значит раздавать секреты парка.
const INSTALL_ONLY_FILES: &[&str] = &["restlex-config.json"];

/// Что отдаётся без ключа при `public: true`: только подписанные программы,
/// в которых нет секретов. restlex-installer*.exe сюда НЕ входят — в нашей
/// сборке в них вшит install_key.
const PUBLIC_FILES: &[&str] = &[
    "restlex.exe", "restlex-agent.exe", "RestLex-Setup.exe", "RestLex-Simple.exe", "versions.json",
];

fn is_public_file(name: &str) -> bool {
    PUBLIC_FILES.iter().any(|w| w.eq_ignore_ascii_case(name))
}

fn is_install_only(name: &str) -> bool {
    INSTALL_ONLY_FILES.iter().any(|w| w.eq_ignore_ascii_case(name))
}

/// Разрешена ли отдача файла: он есть в манифесте либо в белом списке.
fn is_downloadable(name: &str, manifest: &Manifest) -> bool {
    manifest.files.iter().any(|f| f.name == name)
        || DOWNLOADABLE_EXTRA.iter().any(|w| w.eq_ignore_ascii_case(name))
}

struct AppState {
    cfg: Config,
    keys: Vec<String>,
    /// Принимаемые значения `X-Install-Key`. Пусто = установочный конфиг не
    /// отдаётся никому.
    install_keys: Vec<String>,
    /// Кеш манифеста: пересчитывается, только когда меняется состав/размер/mtime
    /// файлов. Раньше каждый GET /api/manifest читал все exe целиком в RAM и
    /// считал SHA-256 прямо в async-хендлере — парк агентов клал сервер.
    manifest: std::sync::Mutex<Option<(DirKey, Arc<Manifest>)>>,
    /// Ограничитель одновременных выгрузок: 100 параллельных загрузок 60-МБ
    /// бинаря раньше означали OOM.
    dl_sem: Arc<tokio::sync::Semaphore>,
}

/// Максимум одновременных отдач файлов по умолчанию (переопределяется полем
/// конфига `max_concurrent_downloads`). 8 было мало: при обновлении парка
/// «поздние» агенты стояли в очереди дольше своего 300-секундного таймаута.
const MAX_CONCURRENT_DOWNLOADS: usize = 32;

/// Сколько ждать места в очереди отдачи, прежде чем ответить 503.
/// Заведомо меньше 300-секундного таймаута агента.
const DOWNLOAD_QUEUE_WAIT_SECS: u64 = 60;

/// Значение заголовка `Retry-After` при 503.
const RETRY_AFTER_SECS: u64 = 30;

#[derive(Serialize)]
struct Manifest {
    agent_version: String,
    gui_version: String,
    files: Vec<FileEntry>,
}

#[derive(Serialize)]
struct FileEntry {
    name: String,
    size: u64,
    sha256: String,
}

/// Manifest для запроса без действующего ключа (public): только PUBLIC_FILES.
/// Агент качает ВСЕ файлы manifest и рвёт цикл на первом 401 — без фильтра
/// установщики/серверные exe в списке ломали обновление агентам без ключа
/// (и старым, чей ключ снят ротацией).
#[derive(Serialize)]
struct PublicManifest<'a> {
    agent_version: &'a str,
    gui_version: &'a str,
    files: Vec<&'a FileEntry>,
}

fn public_view(m: &Manifest) -> PublicManifest<'_> {
    PublicManifest {
        agent_version: &m.agent_version,
        gui_version: &m.gui_version,
        files: m.files.iter().filter(|f| is_public_file(&f.name)).collect(),
    }
}

fn read_versions(folder: &Path) -> (String, String) {
    // versions.json: {"agent": "6.0.2", "gui": "1.0.28"}
    let vjson = folder.join("versions.json");
    if let Ok(s) = fs::read_to_string(&vjson) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            let agent = v.get("agent").and_then(|x| x.as_str()).unwrap_or("0.0.0").to_string();
            let gui   = v.get("gui").and_then(|x| x.as_str()).unwrap_or("0.0.0").to_string();
            if agent != "0.0.0" {
                return (agent, gui);
            }
        }
    }
    // legacy: version.txt → agent only
    let vfile = folder.join("version.txt");
    if let Ok(s) = fs::read_to_string(&vfile) {
        let v = s.trim().to_string();
        if !v.is_empty() {
            return (v, "0.0.0".to_string());
        }
    }
    // fallback: scan exe names for *-X.Y.Z.exe pattern
    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(v) = extract_version_from_name(&name) {
                return (v, "0.0.0".to_string());
            }
        }
    }
    ("0.0.0".to_string(), "0.0.0".to_string())
}

fn extract_version_from_name(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    // match last -X.Y.Z segment
    let parts: Vec<&str> = stem.split('-').collect();
    for part in parts.iter().rev() {
        let segs: Vec<&str> = part.split('.').collect();
        if segs.len() >= 2 && segs.iter().all(|s| s.chars().all(|c| c.is_ascii_digit())) {
            return Some(part.to_string());
        }
    }
    None
}

/// Потоковый SHA-256: файл не читается целиком в память.
fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

fn build_manifest(folder: &Path) -> Result<Manifest> {
    let (agent_version, gui_version) = read_versions(folder);
    let mut files = Vec::new();
    // Канонический путь папки: `download` режет symlink/junction наружу, значит
    // и манифест не должен их перечислять — иначе агент видит файл в манифесте
    // и получает 404 при скачивании.
    let real_folder = fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    for entry in fs::read_dir(folder)?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("exe") {
            continue;
        }
        let Ok(real) = fs::canonicalize(&p) else { continue };
        if !real.starts_with(&real_folder) {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        // Тот же race с deploy.ps1, что и в dir_key: пропускаем исчезнувшее.
        let Ok(md) = p.metadata() else { continue };
        let Ok(sha256) = sha256_file(&p) else { continue };
        files.push(FileEntry { name, size: md.len(), sha256 });
    }
    Ok(Manifest { agent_version, gui_version, files })
}

fn dir_key(folder: &Path) -> Result<DirKey> {
    let mut v: DirKey = Vec::new();
    for entry in fs::read_dir(folder)?.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_exe = p.extension().and_then(|e| e.to_str()) == Some("exe");
        // versions.json определяет agent_version/gui_version в манифесте, а
        // deploy.ps1 пишет его ПОСЛЕ бинарников: без него кеш, снятый в этом
        // окне, навсегда фиксировал новые exe со старыми номерами версий.
        let is_ver = VERSION_FILES.iter().any(|v| v.eq_ignore_ascii_case(&name));
        if !is_exe && !is_ver {
            continue;
        }
        // Файл мог исчезнуть между read_dir и metadata (deploy.ps1 в этот
        // момент переставляет бинари): это не повод ронять весь манифест 500-й
        // и взводить circuit breaker у агентов — пропускаем запись.
        let Ok(md) = entry.metadata() else { continue };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        v.push((name, md.len(), mtime));
    }
    v.sort();
    Ok(v)
}

/// Манифест из кеша; пересчёт (с чтением файлов) — только при изменении папки.
/// Вызывать ТОЛЬКО из spawn_blocking: внутри блокирующий I/O.
fn manifest_cached(st: &AppState) -> Result<Arc<Manifest>> {
    let folder = PathBuf::from(&st.cfg.folder);
    let key = dir_key(&folder)?;
    if let Ok(g) = st.manifest.lock() {
        if let Some((k, m)) = g.as_ref() {
            if *k == key {
                return Ok(m.clone());
            }
        }
    }
    let m = Arc::new(build_manifest(&folder)?);
    if let Ok(mut g) = st.manifest.lock() {
        *g = Some((key, m.clone()));
    }
    Ok(m)
}

/// Сравнение, не зависящее от того, сколько первых байт совпало: сравниваются
/// не сами ключи, а их SHA-256. То же, что `hmac_eq` в web_server.rs.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut ha = Sha256::new();
    ha.update(a);
    let mut hb = Sha256::new();
    hb.update(b);
    ha.finalize() == hb.finalize()
}

/// Список принимаемых ключей: `api_keys` (если задан), иначе legacy-разбор
/// `api_key` по запятой.
fn api_keys(cfg: &Config) -> Vec<String> {
    let explicit: Vec<String> = cfg
        .api_keys
        .iter()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    if !explicit.is_empty() {
        return explicit;
    }
    cfg.api_key
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(String::from)
        .collect()
}

/// Разбор значений `--api-key` при `install`.
///
/// Один аргумент разбирается по запятой — так ротация задокументирована в
/// `--help` и в README (`--api-key "старый,новый"`). Несколько аргументов
/// (`--api-key A --api-key B`) берутся как есть: это единственный способ задать
/// ключ, внутри которого сама запятая.
fn split_api_key_args(args: &[String]) -> Vec<String> {
    let parts: Vec<String> = if args.len() > 1 {
        args.iter().map(|s| s.trim().to_string()).collect()
    } else {
        args.first()
            .map(|s| s.split(',').map(|k| k.trim().to_string()).collect())
            .unwrap_or_default()
    };
    parts.into_iter().filter(|k| !k.is_empty()).collect()
}

/// `expected` — один ключ или несколько через запятую. Несколько нужны на время
/// ротации ключа: клиенты со старым и новым значением работают одновременно,
/// пока все не подтянут обновление. Пробелы вокруг ключей игнорируются, пустые
/// элементы не принимаются (иначе "key," открыл бы доступ по пустому заголовку).
fn check_api_key(headers: &HeaderMap, keys: &[String]) -> bool {
    check_key_header(headers, "X-Api-Key", keys)
}

/// То же сравнение для произвольного заголовка (`X-Install-Key`).
fn check_key_header(headers: &HeaderMap, header: &str, keys: &[String]) -> bool {
    let Some(got) = headers.get(header).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    // Без раннего выхода: перебираются все ключи целиком.
    let mut ok = false;
    for k in keys {
        ok |= ct_eq(k.as_bytes(), got.as_bytes());
    }
    ok
}

async fn manifest(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let keyed = check_api_key(&headers, &st.keys);
    if !st.cfg.public && !keyed {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let st2 = st.clone();
    match tokio::task::spawn_blocking(move || manifest_cached(&st2)).await {
        Ok(Ok(m)) if keyed => Json(&*m).into_response(),
        Ok(Ok(m)) => Json(public_view(&m)).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct DownloadQuery {
    file: String,
}

/// URL query уже percent-decoded. Не нормализуем `../x.exe` к `x.exe`:
/// подозрительное имя должно получить 400, а не ответ из каталога обновлений.
fn safe_download_name(file: &str) -> Option<&str> {
    if file.is_empty() || file == "." || file == ".."
        || file.chars().any(|ch| matches!(ch, '/' | '\\' | ':' | '\0'))
    {
        return None;
    }
    (Path::new(file).file_name().and_then(|name| name.to_str()) == Some(file)).then_some(file)
}

async fn download(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<DownloadQuery>,
) -> Response {
    use tokio_stream::StreamExt as _;
    // Не допускаем ни traversal, ни абсолютный путь, ни альтернативный NTFS stream.
    let Some(name) = safe_download_name(&q.file) else {
        return (StatusCode::BAD_REQUEST, "bad filename").into_response();
    };
    // public: программы без ключа; restlex-config.json дальше всё равно
    // требует X-Install-Key, остальное (установщики, серверные exe) — X-Api-Key.
    let key_needed = !st.cfg.public || !(is_public_file(name) || is_install_only(name));
    if key_needed && !check_api_key(&headers, &st.keys) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    // Отдаём только то, что перечислено в манифесте, плюс явный белый список
    // не-exe (restlex-config.json тянут инсталляторы): в папке обновлений лежат
    // ещё deploy.ps1 и README.txt, а junction/symlink внутри неё читался
    // насквозь.
    if is_install_only(&name) {
        // Установочный конфиг — только по отдельному токену. Пустой install_key
        // в конфиге службы означает «не раздавать»: секреты парка по открытому
        // HTTP не уезжают, установка при этом не падает — инсталлятор трактует
        // отсутствие конфига как предупреждение.
        if st.install_keys.is_empty() {
            tracing::warn!(
                "отказ в {name}: install_key не задан в {CONFIG_PATH} \
                 (добавьте поле install_key и пропишите тот же токен в инсталляторе)"
            );
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
        if !check_key_header(&headers, "X-Install-Key", &st.install_keys) {
            return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
        }
    } else {
        let st2 = st.clone();
        let m = match tokio::task::spawn_blocking(move || manifest_cached(&st2)).await {
            Ok(Ok(m)) => m,
            _ => return (StatusCode::INTERNAL_SERVER_ERROR, "manifest error").into_response(),
        };
        if !is_downloadable(&name, &m) {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
    }
    let Ok(folder) = fs::canonicalize(&st.cfg.folder) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "bad folder").into_response();
    };
    let Ok(path) = fs::canonicalize(folder.join(&name)) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if !path.starts_with(&folder) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    // Ждём место в очереди ограниченно: неограниченное ожидание перекрывало
    // 300-секундный таймаут агента (updater.rs), тот получал «ошибку загрузки»
    // и взводил circuit breaker, который админу приходилось сбрасывать руками.
    // Лучше явный 503 задолго до таймаута.
    let permit = match tokio::time::timeout(
        std::time::Duration::from_secs(DOWNLOAD_QUEUE_WAIT_SECS),
        st.dl_sem.clone().acquire_owned(),
    ).await {
        Ok(Ok(p)) => p,
        // Retry-After: без него клиент не отличает «сервер занят, зайди позже»
        // от фатальной ошибки и рвёт весь манифест на первом же 503.
        _ => {
            return Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header("Retry-After", RETRY_AFTER_SECS.to_string())
                .body(axum::body::Body::from("busy"))
                .unwrap_or_else(|_| (StatusCode::SERVICE_UNAVAILABLE, "busy").into_response())
        }
    };
    let Ok(file) = tokio::fs::File::open(&path).await else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    // Длина берётся из УЖЕ ОТКРЫТОГО хэндла, а не из пути: замена файла в папке
    // (staging→move) оставляет наш хэндл на прежнем содержимом, поэтому
    // Content-Length и поток остаются согласованными.
    let len = file.metadata().await.map(|md| md.len()).unwrap_or(0);
    // Стриминг вместо чтения файла целиком в RAM. permit держится до конца
    // потока — иначе лимит снялся бы сразу после отдачи заголовков.
    let stream = tokio_util::io::ReaderStream::new(file).map(move |chunk| {
        let _hold = &permit;
        chunk
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", len)
        .body(axum::body::Body::from_stream(stream))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "resp").into_response())
}

pub(crate) async fn run_server(cfg: Config) -> Result<()> {
    let addr = format!("0.0.0.0:{}", cfg.port);
    // ponytail: TLS на этой службе и подпись манифеста обновлений — решение
    // владельца; агент запускает полученные бинарники под SYSTEM, так что
    // канал сейчас защищён только общим api_key.
    let keys = api_keys(&cfg);
    let install_keys: Vec<String> = cfg
        .install_key
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(String::from)
        .collect();
    if install_keys.is_empty() {
        tracing::warn!(
            "install_key не задан — restlex-config.json раздаваться НЕ будет \
             (инсталляторы получат 404 и пропустят применение серверного конфига)"
        );
    }
    let max_dl = if cfg.max_concurrent_downloads > 0 {
        cfg.max_concurrent_downloads
    } else {
        MAX_CONCURRENT_DOWNLOADS
    };
    let state = Arc::new(AppState {
        cfg,
        keys,
        install_keys,
        manifest: std::sync::Mutex::new(None),
        dl_sem: Arc::new(tokio::sync::Semaphore::new(max_dl)),
    });
    let app = Router::new()
        .route("/api/manifest", get(manifest))
        .route("/api/download", get(download))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("RestLex Update Server listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Install { port, folder, api_key, install_key, public, agent_version, gui_version } => {
            // `--api-key "old,new"` — исторически задокументированная ротация:
            // строку разбираем по запятой. Повторный `--api-key` даёт элементы
            // без разбиения — так задаётся ключ, содержащий запятую.
            let keys = split_api_key_args(&api_key);
            if keys.is_empty() {
                anyhow::bail!("--api-key: пустое значение");
            }
            let cfg = Config { port, folder: folder.clone(), api_key: String::new(),
                               api_keys: keys, install_key: install_key.trim().to_string(),
                               max_concurrent_downloads: 0,
                               public: public || Config::load().map(|c| c.public).unwrap_or(false) };
            cfg.save()?;
            println!("Конфиг сохранён: {CONFIG_PATH}");

            // Create folder and versions.json
            let folder_path = std::path::Path::new(&folder);
            fs::create_dir_all(folder_path)?;
            let versions_path = folder_path.join("versions.json");
            let versions_json = serde_json::json!({
                "agent": agent_version,
                "gui": gui_version,
            });
            fs::write(&versions_path, serde_json::to_string_pretty(&versions_json)?)?;
            println!("versions.json создан: {}", versions_path.display());
            println!("  agent: {agent_version}  gui: {gui_version}");
            println!("Положите restlex.exe / restlex-agent.exe в папку и обновите versions.json");

            // Create README.txt
            let readme = format!(r#"RestLex Update Server — папка обновлений
==========================================

Сервер раздаёт файлы клиентам (restlex-agent) по HTTP.
Порт: {port}
Конфиг службы: {CONFIG_PATH}

── Структура папки ───────────────────────────────────────
  versions.json          — версии для клиентов (обязательно)
  restlex.exe            — новая версия GUI
  restlex-agent.exe      — новая версия агента (опционально)

── versions.json ────────────────────────────────────────
  {{
    "agent": "6.0.2",    <- должна совпадать с CARGO_PKG_VERSION
    "gui":   "1.0.28"       собранного restlex-agent.exe,
  }}                         иначе агент обновляется в цикле!

  GUI обновляется по SHA-256 (не по номеру версии).
  Достаточно положить новый restlex.exe и обновить "gui" —
  пересобирать агент не нужно.

── API endpoints ─────────────────────────────────────────
  GET /api/manifest           — список файлов + версии
  GET /api/download?file=...  — скачать файл

  Все запросы требуют заголовок: X-Api-Key: <ваш ключ>

── restlex-config.json (установочный конфиг) ─────────────
  Содержит global/profiles парка В ПЛЕЙНТЕКСТЕ, поэтому
  отдаётся НЕ по X-Api-Key, а только по отдельному токену:
    X-Install-Key: <install_key>
  Задаётся при install (--install-key) или полем install_key
  в {CONFIG_PATH}. Не задан — файл не отдаётся (404).
  Тот же токен должен быть зашит в инсталляторе.

── Управление службой ────────────────────────────────────
  restlex-update-server.exe start
  restlex-update-server.exe stop
  restlex-update-server.exe uninstall
"#);
            fs::write(folder_path.join("README.txt"), readme.as_bytes())?;

            #[cfg(windows)]
            service::install()?;
            #[cfg(not(windows))]
            println!("Служба поддерживается только на Windows");
            Ok(())
        }
        Cmd::Uninstall => {
            #[cfg(windows)]
            service::uninstall()?;
            #[cfg(not(windows))]
            println!("Служба поддерживается только на Windows");
            Ok(())
        }
        Cmd::Start => {
            #[cfg(windows)]
            service::start_svc()?;
            Ok(())
        }
        Cmd::Stop => {
            #[cfg(windows)]
            service::stop_svc()?;
            Ok(())
        }
        Cmd::Run => {
            let cfg = Config::load()?;
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(run_server(cfg))
        }
        #[cfg(windows)]
        Cmd::Service => service::dispatch(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_from_name() {
        assert_eq!(
            extract_version_from_name("restlex-agent-6.1.0.exe"),
            Some("6.1.0".to_string())
        );
        assert_eq!(extract_version_from_name("restlex.exe"), None);
    }

    fn hdr(v: &str) -> super::HeaderMap {
        let mut h = super::HeaderMap::new();
        h.insert("X-Api-Key", v.parse().unwrap());
        h
    }

    fn cfg_legacy(k: &str) -> super::Config {
        super::Config { api_key: k.to_string(), ..Default::default() }
    }

    #[test]
    fn api_key_single_and_rotation_list() {
        let ks = super::api_keys;
        // один ключ — как раньше
        assert!(super::check_api_key(&hdr("old"), &ks(&cfg_legacy("old"))));
        assert!(!super::check_api_key(&hdr("new"), &ks(&cfg_legacy("old"))));
        // ротация через legacy-запятую: принимаются оба, пробелы игнорируются
        assert!(super::check_api_key(&hdr("old"), &ks(&cfg_legacy("old, new"))));
        assert!(super::check_api_key(&hdr("new"), &ks(&cfg_legacy("old, new"))));
        assert!(!super::check_api_key(&hdr("other"), &ks(&cfg_legacy("old, new"))));
        // пустой элемент не должен открывать доступ по пустому заголовку
        assert!(!super::check_api_key(&hdr(""), &ks(&cfg_legacy("old,"))));
        assert!(!super::check_api_key(&super::HeaderMap::new(), &ks(&cfg_legacy("old"))));
    }

    /// `--api-key "old,new"` при install обязан дать ДВА принимаемых ключа —
    /// иначе ротация ключа кладёт весь парк в 401.
    #[test]
    fn install_api_key_comma_gives_two_keys() {
        let keys = super::split_api_key_args(&["old, new".to_string()]);
        assert_eq!(keys, vec!["old".to_string(), "new".to_string()]);
        let cfg = super::Config { api_keys: keys, ..Default::default() };
        let ks = super::api_keys(&cfg);
        assert!(super::check_api_key(&hdr("old"), &ks));
        assert!(super::check_api_key(&hdr("new"), &ks));
        assert!(!super::check_api_key(&hdr("old, new"), &ks));
        // Повторный --api-key: запятая внутри ключа сохраняется.
        let keys = super::split_api_key_args(&["aB3,x9Q".to_string(), "second".to_string()]);
        assert_eq!(keys, vec!["aB3,x9Q".to_string(), "second".to_string()]);
    }

    #[test]
    fn public_manifest_lists_only_public_files() {
        let e = |n: &str| FileEntry { name: n.into(), size: 1, sha256: String::new() };
        let m = Manifest { agent_version: "1".into(), gui_version: "2".into(),
            files: vec![e("restlex-agent.exe"), e("restlex-installer-klik.exe"),
                        e("restlex.exe"), e("restlex-webserver.exe")] };
        let names: Vec<_> = public_view(&m).files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["restlex-agent.exe", "restlex.exe"]);
    }

    #[test]
    fn public_files_exclude_installers_and_config() {
        assert!(is_public_file("restlex-agent.exe") && is_public_file("RESTLEX.EXE"));
        assert!(!is_public_file("restlex-installer-klik.exe"));
        assert!(!is_public_file("restlex-installer.exe"));
        assert!(!is_public_file("restlex-config.json"));
        assert!(!is_public_file("restlex-update-server.exe"));
    }

    #[test]
    fn download_rejects_traversal_and_windows_paths() {
        for bad in ["", ".", "..", "../restlex-agent.exe", "..\\restlex-agent.exe",
                    "/restlex-agent.exe", "C:\\restlex-agent.exe",
                    "\\\\server\\share\\restlex-agent.exe", "agent.exe:payload",
                    "folder/subdir/file.exe", "agent.exe\0other"] {
            assert!(super::safe_download_name(bad).is_none(), "{bad:?} unexpectedly allowed");
        }
        for good in ["restlex-agent.exe", "versions.json", "restlex-config.json"] {
            assert_eq!(super::safe_download_name(good), Some(good));
        }
    }

    /// В манифесте только *.exe; versions.json — по обычному ключу,
    /// restlex-config.json — только по install_key (не через is_downloadable).
    #[test]
    fn downloadable_whitelist() {
        let m = super::Manifest {
            agent_version: "6.0.0".into(),
            gui_version: "1.0.0".into(),
            files: vec![super::FileEntry {
                name: "restlex-agent.exe".into(), size: 1, sha256: String::new(),
            }],
        };
        assert!(super::is_downloadable("restlex-agent.exe", &m));
        assert!(super::is_downloadable("versions.json", &m));
        // Секретный установочный конфиг по обычному ключу больше не отдаётся.
        assert!(!super::is_downloadable("restlex-config.json", &m));
        assert!(super::is_install_only("restlex-config.json"));
        assert!(super::is_install_only("RestLex-Config.JSON"));
        assert!(!super::is_install_only("restlex-agent.exe"));
        // Пустой install_key = никому: check_key_header по пустому списку.
        let mut h = super::HeaderMap::new();
        h.insert("X-Install-Key", "whatever".parse().unwrap());
        assert!(!super::check_key_header(&h, "X-Install-Key", &[]));
        assert!(super::check_key_header(&h, "X-Install-Key", &["whatever".to_string()]));
        // Всё остальное вне манифеста — по-прежнему 404.
        assert!(!super::is_downloadable("deploy.ps1", &m));
        assert!(!super::is_downloadable("README.txt", &m));
        assert!(!super::is_downloadable("secrets.txt", &m));
    }

    /// Кеш манифеста обязан инвалидироваться, если изменился ТОЛЬКО
    /// versions.json (deploy.ps1 пишет его после бинарников).
    #[test]
    fn dir_key_tracks_versions_json() {
        let dir = std::env::temp_dir().join(format!("rus-dirkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("restlex-agent.exe"), b"binary").unwrap();
        std::fs::write(dir.join("versions.json"), br#"{"agent":"6.0.1","gui":"1.0.1"}"#).unwrap();
        let k1 = super::dir_key(&dir).unwrap();
        // Тот же размер, другое содержимое — на секундной точности mtime это
        // сходило за «папка не менялась». (Гранулярность системного таймера
        // файловых времён — десятки мс, поэтому короткая пауза.)
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(dir.join("versions.json"), br#"{"agent":"6.0.2","gui":"1.0.2"}"#).unwrap();
        let k2 = super::dir_key(&dir).unwrap();
        assert_ne!(k1, k2, "dir_key не заметил изменения versions.json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ключ с запятой внутри остаётся ОДНИМ ключом, если задан через api_keys.
    #[test]
    fn api_keys_array_is_not_split_on_comma() {
        let cfg = super::Config { api_keys: vec!["aB3,x9Q".into()], ..Default::default() };
        let keys = super::api_keys(&cfg);
        assert!(super::check_api_key(&hdr("aB3,x9Q"), &keys));
        assert!(!super::check_api_key(&hdr("aB3"), &keys));
        assert!(!super::check_api_key(&hdr("x9Q"), &keys));
    }
}

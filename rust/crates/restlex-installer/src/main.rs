// RestLex Installer
// CLI: install --all [--user <name>] [--shortcut-all]
//      update | uninstall | status

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

// ── Вшитые константы ──────────────────────────────────────────────────────────
const SERVER: &str  = "http://1221.by:3266";
// Ключи не в исходниках: build.ps1 задаёт их переменными окружения при сборке
// (наша сборка для клиентов). Публичная сборка — без ключей.
fn api_key() -> String {
    option_env!("RESTLEX_UPDATE_KEY").unwrap_or("").to_string()
}

/// ОТДЕЛЬНЫЙ токен для restlex-config.json (заголовок X-Install-Key).
/// Ключ бинарников зашит в каждый агент парка — раздавать по нему плейнтекстовый
/// конфиг с repo_password/tg_token нельзя. На сервере тот же токен задаётся
/// полем `install_key` в update-server.json (или `--install-key` при install);
/// не задан — сервер отдаёт 404, установка продолжается без серверного конфига.
/// ОГРАНИЧЕНИЕ: в нашей сборке токен вшит в инсталлятор и едет по HTTP без TLS. Он
/// отвязывает выдачу конфига от ключа обновлений и делает её ротируемой, но не
/// заменяет TLS.
fn install_key() -> String {
    option_env!("RESTLEX_INSTALL_KEY").unwrap_or("").to_string()
}

// ── Пути ──────────────────────────────────────────────────────────────────────
fn data_dir() -> PathBuf {
    PathBuf::from(r"C:\ProgramData\RestLex")
}
fn restlex_exe() -> PathBuf { data_dir().join("restlex.exe") }
fn agent_exe()   -> PathBuf { data_dir().join("restlex-agent.exe") }
fn config_json()   -> PathBuf { data_dir().join("config.json") }
fn profiles_json() -> PathBuf { data_dir().join("profiles.json") }
fn state_file()  -> PathBuf { data_dir().join("installer.json") }

/// tmp + rename: падение посреди записи не оставляет обрезанный JSON,
/// который агент при следующем старте молча заменил бы дефолтами.
fn write_json_atomic(p: &Path, s: &str) -> std::io::Result<()> {
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&tmp, s)?;
    fs::rename(&tmp, p)
}

// ── State ─────────────────────────────────────────────────────────────────────
#[derive(Serialize, Deserialize, Default)]
struct State {
    gui_version:   String,
    agent_version: String,
}

fn load_state() -> State {
    fs::read_to_string(state_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(gui: &str, agent: &str) -> Result<()> {
    fs::create_dir_all(data_dir())?;
    let s = serde_json::to_string_pretty(&State {
        gui_version:   gui.to_owned(),
        agent_version: agent.to_owned(),
    })?;
    fs::write(state_file(), s)?;
    Ok(())
}

// ── Manifest ──────────────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct Manifest {
    gui_version:   String,
    agent_version: String,
}

fn fetch_manifest(client: &reqwest::blocking::Client) -> Result<Manifest> {
    let url = format!("{}/api/manifest", SERVER);
    let m: Manifest = client.get(&url)
        .header("X-Api-Key", api_key())
        .send().context("Нет связи с сервером")?
        .error_for_status().context("Сервер вернул ошибку")?
        .json().context("Не удалось разобрать манифест")?;
    Ok(m)
}

fn apply_server_config(client: &reqwest::blocking::Client) -> Result<()> {
    // Публичная сборка: ключа установки нет — настройки организации не тянем.
    if install_key().is_empty() {
        return Ok(());
    }
    let url = format!("{}/api/download?file=restlex-config.json", SERVER);
    let resp = client.get(&url)
        .header("X-Api-Key", api_key())
        .header("X-Install-Key", install_key())
        .send().context("Ошибка соединения с сервером")?
        .error_for_status().context("Сервер вернул ошибку")?
        .text().context("Ошибка чтения конфига")?;
    let mut root: serde_json::Value = serde_json::from_str(&resp)
        .context("Не удалось разобрать restlex-config.json")?;
    let profiles = root.get("profiles").cloned().unwrap_or(serde_json::Value::Object(Default::default()));
    let global   = root.get_mut("global").map(|v| v.take())
        .unwrap_or(serde_json::Value::Object(Default::default()));
    fs::create_dir_all(data_dir())?;
    write_json_atomic(&config_json(),   &serde_json::to_string_pretty(&global)?)?;
    write_json_atomic(&profiles_json(), &serde_json::to_string_pretty(&profiles)?)?;
    Ok(())
}

fn download_file(client: &reqwest::blocking::Client, name: &str, dest: &Path) -> Result<()> {
    let url = format!("{}/api/download?file={}", SERVER, name);
    let mut resp = client.get(&url)
        .header("X-Api-Key", api_key())
        .send().context("Ошибка соединения с сервером")?
        .error_for_status().context("Сервер вернул ошибку")?;

    let parent = dest.parent().unwrap();
    fs::create_dir_all(parent)
        .with_context(|| format!("Не удалось создать папку {}", parent.display()))?;

    let tmp = dest.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("Не удалось создать временный файл {}", tmp.display()))?;
        let mut buf = vec![0u8; 65536];
        loop {
            use std::io::Read;
            let n = resp.read(&mut buf).context("Ошибка чтения данных")?;
            if n == 0 { break; }
            f.write_all(&buf[..n]).context("Ошибка записи")?;
        }
    }
    fs::rename(&tmp, dest)
        .with_context(|| format!("Не удалось переместить {} -> {}", tmp.display(), dest.display()))?;

    // Файлы едут по HTTP без TLS. Свои бинари подписаны EV-сертификатом —
    // проверяем подпись до того, как exe будет запущен/подхвачен службой.
    if dest.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
        if let Err(e) = verify_authenticode(dest) {
            let _ = fs::remove_file(dest);
            return Err(e);
        }
    }
    Ok(())
}

/// SERIALNUMBER'ы в Subject наших EV-сертификатов — тот же список, что в
/// restlex-agent::updater (ООО СофтИнноватиксПлюс). Сверяем по SERIALNUMBER:
/// он в подписи ASCII и не зависит от кодовой страницы консоли, в отличие от
/// кириллического CN. Новый серийник добавляется сюда ЗАРАНЕЕ, до перехода на
/// новый сертификат.
const ALLOWED_SIGNER_SERIALS: &[&str] = &["SERIALNUMBER=193808466"];

/// Проверка Authenticode: MITM на HTTP подсовывает свой exe, который тут же
/// запускается с правами администратора.
///
/// fail-OPEN ТОЛЬКО когда проверку физически не выполнили (powershell не
/// запустился / не вернул ничего осмысленного): без этого урезанный образ или
/// заблокированный AppLocker'ом PS ломал бы установку там, где раньше работала.
/// ЛЮБОЙ полученный статус ≠ Valid — отказ (fail-closed), включая NotTrusted и
/// UnknownError: именно их даёт самоподписанный exe от MITM. Плюс пиннинг
/// подписанта: `Valid` от чужого CA не принимается.
fn verify_authenticode(path: &Path) -> Result<()> {
    // ВНИМАНИЕ: скрипт обязан работать в PowerShell 2.0 (штатный на Win7/2008R2):
    // без -LiteralPath, без ??/?:, без ConvertFrom-Json.
    let ps = format!(
        "$ErrorActionPreference='SilentlyContinue'; \
         $s=Get-AuthenticodeSignature -FilePath '{}'; \
         if($s -eq $null){{'ERR'}}else{{'ST:'+$s.Status+'|SUBJ:'+$s.SignerCertificate.Subject}}",
        ps_quote(&path.to_string_lossy())
    );
    let out = match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .creation_flags(0x08000000)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            println!("  ВНИМАНИЕ: подпись {} не проверена (powershell не запустился: {e}) — продолжаю",
                     path.display());
            return Ok(());
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if stdout.is_empty() || stdout == "ERR" {
        println!("  ВНИМАНИЕ: подпись {} не проверена (powershell не вернул статус, rc={:?}) — продолжаю",
                 path.display(), out.status.code());
        return Ok(());
    }
    let status = stdout
        .strip_prefix("ST:")
        .and_then(|s| s.split('|').next())
        .unwrap_or("")
        .trim()
        .to_owned();
    let subject = stdout.split("|SUBJ:").nth(1).unwrap_or("").trim().to_owned();
    if status.is_empty() {
        println!("  ВНИМАНИЕ: подпись {} не проверена (непонятный ответ powershell: {stdout}) — продолжаю",
                 path.display());
        return Ok(());
    }
    // Дальше powershell ЯВНО ответил — это уже суждение о подписи, fail-closed.
    if status != "Valid" {
        bail!("{}: подпись не валидна (статус {}), файл отброшен", path.display(), status);
    }
    if !ALLOWED_SIGNER_SERIALS.iter().any(|s| subject.contains(s)) {
        bail!("{}: чужой подписант ({}), файл отброшен", path.display(), subject);
    }
    Ok(())
}

/// ProgramData наследует ACL, где у Users есть право записи, а служба под SYSTEM
/// грузит отсюда restlex-agent.exe и читает config.json/profiles.json. Режем
/// наследование: полный доступ только SYSTEM и Администраторам, остальным — RX.
fn harden_data_dir() {
    let _ = Command::new("icacls")
        .arg(data_dir().as_os_str())
        .args([
            "/inheritance:r",
            "/grant", "*S-1-5-18:(OI)(CI)F",      // SYSTEM
            "/grant", "*S-1-5-32-544:(OI)(CI)F",  // Administrators
            "/grant", "*S-1-5-32-545:(OI)(CI)RX", // Users — только чтение/запуск
        ])
        .creation_flags(0x08000000)
        .status();
    harden_secure_dir();
    harden_secret_files();
}

/// Подкаталог для секретов: наследование срезано, Users в нём нет вообще.
/// Любой файл, созданный ЗДЕСЬ (в том числе через `<имя>.tmp` + rename, как
/// пишут `_atomic_write_json` в Python и `write_atomic` в Rust), унаследует
/// закрытый ACL — в отличие от config.json в корне, где переименование стирает
/// точечный DACL и возвращает Users:RX. Корень закрыть нельзя: restlex.exe
/// обязан быть читаем ФИЛЬТРОВАННЫМ токеном, иначе ярлык не запускается даже у
/// администратора (UAC читает манифест до повышения).
fn harden_secure_dir() {
    let dir = data_dir().join("secure");
    let _ = fs::create_dir_all(&dir);
    let _ = Command::new("icacls")
        .arg(dir.as_os_str())
        .args([
            "/inheritance:r",
            "/grant", "*S-1-5-18:(OI)(CI)F",     // SYSTEM
            "/grant", "*S-1-5-32-544:(OI)(CI)F", // Administrators
        ])
        .creation_flags(0x08000000)
        .status();
}

/// У каталога Users оставлены RX — а секреты в config.json/profiles.json
/// зашифрованы DPAPI LOCAL_MACHINE без entropy: кто может ПРОЧИТАТЬ файл, тот
/// расшифрует repo_password/tg_token/api-ключи парой строк кода. Поэтому этим
/// двум файлам режем наследование отдельно: только SYSTEM и Администраторы.
/// GUI работает под админом (uac_admin), служба — под SYSTEM: оба покрыты.
fn harden_secret_files() {
    for name in ["config.json", "profiles.json"] {
        let p = data_dir().join(name);
        // Файла может ещё не быть (первая установка, конфиг создаётся позже) —
        // icacls на несуществующий путь вернёт ошибку, установку это ронять не должно.
        if !p.exists() { continue; }
        let _ = Command::new("icacls")
            .arg(p.as_os_str())
            .args([
                "/inheritance:r",
                "/grant", "*S-1-5-18:F",     // SYSTEM
                "/grant", "*S-1-5-32-544:F", // Administrators
            ])
            .creation_flags(0x08000000)
            .status();
    }
}

// ── Ver compare ───────────────────────────────────────────────────────────────
fn ver_tuple(v: &str) -> (u32, u32, u32) {
    let mut p = v.splitn(3, '.');
    let a = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let b = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let c = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (a, b, c)
}

fn run_silent(exe: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new(exe)
        .args(args)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .status()?;
    if !status.success() {
        bail!("Команда завершилась с кодом {:?}", status.code());
    }
    Ok(())
}

/// Экранирование одинарных кавычек для PowerShell-строки: имя пользователя
/// приходит из --user, и без удвоения `'` значение вида `x';calc;#` выполняется
/// как код в контексте установщика (админ).
fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

fn create_shortcut(lnk: &str) {
    let exe = restlex_exe().to_string_lossy().into_owned();
    let ps  = format!(
        "$s=(New-Object -COM WScript.Shell).CreateShortcut('{lnk}');\
         $s.TargetPath='{exe}';\
         $s.WorkingDirectory='{dir}';\
         $s.IconLocation='{exe},0';\
         $s.Save()",
        lnk = ps_quote(lnk), exe = ps_quote(&exe),
        dir = ps_quote(&data_dir().to_string_lossy())
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .creation_flags(0x08000000)
        .status();
}

fn create_public_shortcut() {
    create_shortcut(r"C:\Users\Public\Desktop\Restlex.lnk");
}

fn create_user_shortcut(username: &str) {
    for desk in &["Desktop", "\u{0420}\u{0430}\u{0431}\u{043e}\u{0447}\u{0438}\u{0439} \u{0441}\u{0442}\u{043e}\u{043b}"] {
        let lnk = format!(r"C:\Users\{}\{}\Restlex.lnk", username, desk);
        let path = std::path::Path::new(&lnk);
        if path.parent().map(|p| p.exists()).unwrap_or(false) {
            create_shortcut(&lnk);
            return;
        }
    }
}

// ── Остановка / запуск службы ─────────────────────────────────────────────────
fn stop_service() {
    println!("Остановка службы RestlexAgent...");
    let _ = Command::new("sc.exe").args(["stop", "RestlexAgent"])
        .creation_flags(0x08000000).status();
    std::thread::sleep(std::time::Duration::from_secs(3));
}

fn kill_process(exe_name: &str) {
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", exe_name])
        .creation_flags(0x08000000).status();
    std::thread::sleep(std::time::Duration::from_millis(500));
}

// ── Команды ───────────────────────────────────────────────────────────────────
fn cmd_install(user: &str, shortcut_all: bool) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    println!("Подключение к серверу...");
    let m = fetch_manifest(&client)?;

    let state = load_state();
    if !state.gui_version.is_empty() {
        println!("Установлено:  GUI {}, агент {}", state.gui_version, state.agent_version);
    }
    println!("На сервере:   GUI {}, агент {}", m.gui_version, m.agent_version);

    let need_gui     = state.gui_version   != m.gui_version   || !restlex_exe().exists();
    let need_agent   = state.agent_version != m.agent_version || !agent_exe().exists();
    let svc_installed = Command::new("sc.exe").args(["query", "RestlexAgent"])
        .creation_flags(0x08000000).output()
        .map(|o| o.status.success()).unwrap_or(false);

    if !need_gui && !need_agent && svc_installed {
        println!("Уже установлено и актуально.");
        return Ok(());
    }

    // Остановить службу и процессы перед перезаписью файлов
    if need_gui || need_agent {
        stop_service();
        if need_gui   { kill_process("restlex.exe"); }
        if need_agent { kill_process("restlex-agent.exe"); }
    }

    fs::create_dir_all(data_dir())?;
    harden_data_dir();

    if need_gui {
        println!("Загрузка restlex.exe...");
        download_file(&client, "restlex.exe", &restlex_exe())?;
    }

    if need_agent {
        println!("Загрузка restlex-agent.exe...");
        download_file(&client, "restlex-agent.exe", &agent_exe())?;
    }

    // Проверяем ДО install — после install restlex.exe создаст дефолтный конфиг
    let need_config = !config_json().exists();

    println!("Установка службы RestlexAgent...");
    if !user.is_empty() {
        run_silent(&restlex_exe(), &["install", "--all", "--user", user])?;
    } else {
        run_silent(&restlex_exe(), &["install", "--all"])?;
    }

    if need_config {
        // Служба уже запущена install-ом с дефолтным конфигом — стопаем, применяем наш, стартуем
        println!("Загрузка конфигурации...");
        let _ = Command::new("sc.exe").args(["stop", "RestlexAgent"])
            .creation_flags(0x08000000).status();
        std::thread::sleep(std::time::Duration::from_secs(2));
        match apply_server_config(&client) {
            Ok(_) => {
                println!("  config.json + profiles.json применены.");
                let _ = Command::new("sc.exe").args(["start", "RestlexAgent"])
                    .creation_flags(0x08000000).status();
            }
            Err(e) => println!("  ПРЕДУПРЕЖДЕНИЕ: конфиг не загружен: {:#}", e),
        }
    } else {
        println!("Конфиг уже есть, пропуск.");
    }

    if shortcut_all {
        println!("Создание ярлыка для всех пользователей...");
        create_public_shortcut();
    } else if !user.is_empty() {
        println!("Создание ярлыка для пользователя {}...", user);
        create_user_shortcut(user);
    }

    // Ещё раз — уже после того, как config.json/profiles.json появились.
    harden_secret_files();

    save_state(&m.gui_version, &m.agent_version)?;
    println!("Установка завершена. GUI {} / агент {}", m.gui_version, m.agent_version);
    Ok(())
}

fn cmd_update() -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    println!("Проверка обновлений...");
    let m     = fetch_manifest(&client)?;
    let state = load_state();

    let newer_gui   = ver_tuple(&m.gui_version)   > ver_tuple(&state.gui_version);
    let newer_agent = ver_tuple(&m.agent_version) > ver_tuple(&state.agent_version);

    if !newer_gui && !newer_agent {
        println!("Установлена актуальная версия (GUI {}, агент {}).",
            state.gui_version, state.agent_version);
        return Ok(());
    }

    println!("GUI {} → {}  |  агент {} → {}",
        state.gui_version, m.gui_version,
        state.agent_version, m.agent_version);

    let _ = Command::new(&agent_exe()).args(["stop"])
        .creation_flags(0x08000000).status();

    if newer_gui {
        println!("Загрузка restlex.exe...");
        download_file(&client, "restlex.exe", &restlex_exe())?;
    }
    if newer_agent {
        println!("Загрузка restlex-agent.exe...");
        download_file(&client, "restlex-agent.exe", &agent_exe())?;
        let _ = Command::new(&agent_exe()).args(["install"])
            .creation_flags(0x08000000).status();
        let _ = Command::new(&agent_exe()).args(["start"])
            .creation_flags(0x08000000).status();
    }

    save_state(&m.gui_version, &m.agent_version)?;
    println!("Обновление завершено.");
    Ok(())
}

fn cmd_uninstall() -> Result<()> {
    if restlex_exe().exists() {
        println!("Удаление службы...");
        let _ = Command::new(&restlex_exe()).args(["--uninstall-agent"])
            .creation_flags(0x08000000).status();
    }
    println!("Удаление файлов...");
    for f in [restlex_exe(), agent_exe(), state_file()] {
        let _ = fs::remove_file(&f);
    }
    // Shortcuts
    let _ = fs::remove_file(r"C:\Users\Public\Desktop\Restlex.lnk");
    let users = PathBuf::from(r"C:\Users");
    if let Ok(rd) = fs::read_dir(&users) {
        for entry in rd.flatten() {
            for desk in ["Desktop", "Рабочий стол"] {
                let lnk = entry.path().join(desk).join("Restlex.lnk");
                let _ = fs::remove_file(&lnk);
            }
        }
    }
    let _ = fs::remove_file(state_file());
    println!("Удалено.");
    Ok(())
}

fn cmd_status() -> Result<()> {
    let state     = load_state();
    let installed = restlex_exe().exists();

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let server = fetch_manifest(&client)
        .map(|m| format!("GUI {}  агент {}", m.gui_version, m.agent_version))
        .unwrap_or_else(|_| "недоступен".to_owned());

    println!("Установлено:   {}", if installed { "да" } else { "нет" });
    println!("GUI версия:    {}", state.gui_version);
    println!("Агент версия:  {}", state.agent_version);
    println!("На сервере:    {}", server);
    Ok(())
}

// ── main ──────────────────────────────────────────────────────────────────────
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let result = match args.first().map(String::as_str) {
        Some("install") => {
            let user         = args.iter().position(|a| a == "--user")
                .and_then(|i| args.get(i + 1))
                .map(String::as_str)
                .unwrap_or("");
            let shortcut_all = args.contains(&"--shortcut-all".to_owned());
            cmd_install(user, shortcut_all)
        }
        Some("update")    => cmd_update(),
        Some("uninstall") => cmd_uninstall(),
        Some("status")    => cmd_status(),
        _ => {
            eprintln!("RestLex Installer\n");
            eprintln!("  install --all [--user <name>] [--shortcut-all]");
            eprintln!("  update");
            eprintln!("  uninstall");
            eprintln!("  status");
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("ОШИБКА: {:#}", e);
        std::process::exit(1);
    }
}

// Нужен для CREATE_NO_WINDOW на Windows
trait CommandExt {
    fn creation_flags(&mut self, flags: u32) -> &mut Self;
}
impl CommandExt for Command {
    fn creation_flags(&mut self, flags: u32) -> &mut Self {
        use std::os::windows::process::CommandExt as WinExt;
        WinExt::creation_flags(self, flags)
    }
}

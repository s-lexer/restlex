//! restlex-installer-klik — GUI-инсталлятор RestLex с вшитым ключом API
#![windows_subsystem = "windows"]

use std::{
    collections::VecDeque,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicIsize, Ordering},
        Mutex, OnceLock,
    },
    thread,
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{InitCommonControlsEx, INITCOMMONCONTROLSEX, ICC_PROGRESS_CLASS},
        WindowsAndMessaging::{
            BN_CLICKED, CreateWindowExW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
            DefWindowProcW, DispatchMessageW, GetMessageW, IDC_ARROW, LoadCursorW, MSG,
            PostMessageW, PostQuitMessage, RegisterClassExW, SendMessageW, ShowWindow,
            SW_SHOWDEFAULT, TranslateMessage, WM_APP, WM_COMMAND, WM_CREATE, WM_DESTROY,
            WM_PAINT, WNDCLASSEXW,
        },
    },
};

// EnableWindow и UpdateWindow объявляем вручную (расположение в windows-sys 0.52 зависит от фич)
extern "system" {
    fn EnableWindow(hWnd: HWND, bEnable: i32) -> i32;
    fn UpdateWindow(hWnd: HWND) -> i32;
}

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
/// ОГРАНИЧЕНИЕ: в нашей сборке токен вшит в инсталлятор и едет по HTTP без TLS.
fn install_key() -> String {
    option_env!("RESTLEX_INSTALL_KEY").unwrap_or("").to_string()
}
const SERVER: &str = "http://1221.by:3266";

// ── Сырые значения сообщений Windows ─────────────────────────────────────────
const WS_OVERLAPPED: u32     = 0x0000_0000;
const WS_CAPTION: u32        = 0x00C0_0000;
const WS_SYSMENU: u32        = 0x0008_0000;
const WS_MINIMIZEBOX: u32    = 0x0002_0000;
const WS_CHILD: u32          = 0x4000_0000;
const WS_VISIBLE: u32        = 0x1000_0000;
const WS_VSCROLL: u32        = 0x0020_0000;
const WS_EX_CLIENTEDGE: u32  = 0x0000_0200;
const ES_MULTILINE: u32      = 0x0000_0004;
const ES_READONLY: u32       = 0x0000_0800;
const ES_AUTOVSCROLL: u32    = 0x0000_0040;

const WM_GETTEXTLENGTH: u32  = 0x000E;
const EM_SETSEL: u32         = 0x00B1;
const EM_REPLACESEL: u32     = 0x00C2;
const EM_SCROLLCARET: u32    = 0x00B7;
const PBM_SETRANGE32: u32    = 0x0406;
const PBM_SETPOS: u32        = 0x0402;
const CREATE_NO_WINDOW: u32  = 0x0800_0000;

const BS_PUSHBUTTON: u32     = 0x0000_0000;
const WM_WORKER_LOG: u32     = WM_APP + 1;
const WM_WORKER_DONE: u32    = WM_APP + 2;

const ID_LOG: usize          = 103;
const ID_PROG: usize         = 104;
const ID_INST: usize         = 101;
const ID_REM: usize          = 102;

// ── Глобальные хэндлы (заполняются в WM_CREATE) ───────────────────────────────
static HWND_MAIN: AtomicIsize = AtomicIsize::new(0);
static HWND_LOG:  AtomicIsize = AtomicIsize::new(0);
static HWND_PROG: AtomicIsize = AtomicIsize::new(0);
static HWND_INST: AtomicIsize = AtomicIsize::new(0);
static HWND_REM:  AtomicIsize = AtomicIsize::new(0);
static BUSY:      AtomicBool  = AtomicBool::new(false);

fn log_queue() -> &'static Mutex<VecDeque<String>> {
    static Q: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(VecDeque::new()))
}

// ── Утилиты ───────────────────────────────────────────────────────────────────
fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn append_log(hwnd_edit: HWND, line: &str) {
    unsafe {
        let len = SendMessageW(hwnd_edit, WM_GETTEXTLENGTH, 0, 0) as usize;
        SendMessageW(hwnd_edit, EM_SETSEL, len, len as isize);
        let text = format!("{}\r\n", line);
        SendMessageW(hwnd_edit, EM_REPLACESEL, 0, w(&text).as_ptr() as isize);
        SendMessageW(hwnd_edit, EM_SCROLLCARET, 0, 0);
    }
}

fn post_log(hwnd: HWND, s: String) {
    if let Ok(mut q) = log_queue().lock() {
        q.push_back(s);
    }
    unsafe { PostMessageW(hwnd, WM_WORKER_LOG, 0, 0) };
}

fn post_done(hwnd: HWND, ok: bool) {
    unsafe { PostMessageW(hwnd, WM_WORKER_DONE, ok as usize, 0) };
}

// ── Пути установки ────────────────────────────────────────────────────────────
fn data_dir()      -> PathBuf { PathBuf::from(r"C:\ProgramData\RestLex") }
fn restlex_exe()   -> PathBuf { data_dir().join("restlex.exe") }
fn agent_exe()     -> PathBuf { data_dir().join("restlex-agent.exe") }
fn config_json()   -> PathBuf { data_dir().join("config.json") }
fn profiles_json() -> PathBuf { data_dir().join("profiles.json") }
fn state_file()    -> PathBuf { data_dir().join("installer.json") }

/// tmp + rename: падение посреди записи не оставляет обрезанный JSON.
fn write_json_atomic(p: &std::path::Path, s: &str) -> std::io::Result<()> {
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, s)?;
    std::fs::rename(&tmp, p)
}

// ── HTTP-утилиты ──────────────────────────────────────────────────────────────
fn download_file(
    client: &reqwest::blocking::Client,
    name: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let url = format!("{}/api/download?file={}", SERVER, name);
    let mut resp = client
        .get(&url)
        .header("X-Api-Key", api_key())
        .send()?
        .error_for_status()?;
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let tmp = dest.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        let mut buf = vec![0u8; 65536];
        loop {
            let n = resp.read(&mut buf)?;
            if n == 0 { break; }
            f.write_all(&buf[..n])?;
        }
    }
    std::fs::rename(&tmp, dest)?;

    // Файлы едут по HTTP без TLS. Свои бинари подписаны EV-сертификатом —
    // проверяем подпись до того, как exe будет запущен/подхвачен службой.
    if dest.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
        if let Err(e) = verify_authenticode(dest) {
            let _ = std::fs::remove_file(dest);
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
/// запустился / не вернул ничего осмысленного). ЛЮБОЙ полученный статус ≠ Valid —
/// отказ (fail-closed), включая NotTrusted и UnknownError: именно их даёт
/// самоподписанный exe от MITM. Плюс пиннинг подписанта: `Valid` от чужого CA
/// не принимается.
fn verify_authenticode(path: &Path) -> anyhow::Result<()> {
    let warn = |msg: String| {
        let hwnd = HWND_MAIN.load(Ordering::SeqCst);
        if hwnd == 0 {
            if let Ok(mut q) = log_queue().lock() { q.push_back(msg); }
        } else {
            post_log(hwnd as HWND, msg);
        }
    };
    // ВНИМАНИЕ: скрипт обязан работать в PowerShell 2.0 (штатный на Win7/2008R2).
    let ps = format!(
        "$ErrorActionPreference='SilentlyContinue'; \
         $s=Get-AuthenticodeSignature -FilePath '{}'; \
         if($s -eq $null){{'ERR'}}else{{'ST:'+$s.Status+'|SUBJ:'+$s.SignerCertificate.Subject}}",
        ps_quote(&path.to_string_lossy())
    );
    let out = match std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            warn(format!("  ВНИМАНИЕ: подпись не проверена (powershell не запустился: {e}) — продолжаю"));
            return Ok(());
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if stdout.is_empty() || stdout == "ERR" {
        warn(format!(
            "  ВНИМАНИЕ: подпись не проверена (powershell не вернул статус, rc={:?}) — продолжаю",
            out.status.code()
        ));
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
        warn(format!("  ВНИМАНИЕ: подпись не проверена (непонятный ответ powershell: {stdout}) — продолжаю"));
        return Ok(());
    }
    // Дальше powershell ЯВНО ответил — это уже суждение о подписи, fail-closed.
    if status != "Valid" {
        anyhow::bail!("{}: подпись не валидна (статус {}), файл отброшен", path.display(), status);
    }
    if !ALLOWED_SIGNER_SERIALS.iter().any(|s| subject.contains(s)) {
        anyhow::bail!("{}: чужой подписант ({}), файл отброшен", path.display(), subject);
    }
    Ok(())
}

/// ProgramData наследует ACL, где у Users есть право записи, а служба под SYSTEM
/// грузит отсюда restlex-agent.exe и читает config.json/profiles.json. Режем
/// наследование: полный доступ только SYSTEM и Администраторам, остальным — RX.
fn harden_data_dir() {
    let _ = std::process::Command::new("icacls")
        .arg(data_dir().as_os_str())
        .args([
            "/inheritance:r",
            "/grant", "*S-1-5-18:(OI)(CI)F",      // SYSTEM
            "/grant", "*S-1-5-32-544:(OI)(CI)F",  // Administrators
            "/grant", "*S-1-5-32-545:(OI)(CI)RX", // Users — только чтение/запуск
        ])
        .creation_flags(CREATE_NO_WINDOW)
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
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::process::Command::new("icacls")
        .arg(dir.as_os_str())
        .args([
            "/inheritance:r",
            "/grant", "*S-1-5-18:(OI)(CI)F",     // SYSTEM
            "/grant", "*S-1-5-32-544:(OI)(CI)F", // Administrators
        ])
        .creation_flags(CREATE_NO_WINDOW)
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
        let _ = std::process::Command::new("icacls")
            .arg(p.as_os_str())
            .args([
                "/inheritance:r",
                "/grant", "*S-1-5-18:F",     // SYSTEM
                "/grant", "*S-1-5-32-544:F", // Administrators
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
}

fn apply_server_config(client: &reqwest::blocking::Client) -> anyhow::Result<()> {
    // Публичная сборка: ключа установки нет — настройки организации не тянем.
    if install_key().is_empty() {
        return Ok(());
    }
    let url = format!("{}/api/download?file=restlex-config.json", SERVER);
    let text = client.get(&url)
        .header("X-Api-Key", api_key())
        .header("X-Install-Key", install_key())
        .send()?.error_for_status()?.text()?;
    let mut root: serde_json::Value = serde_json::from_str(&text)?;
    let profiles = root.get("profiles").cloned().unwrap_or_default();
    let global   = root.get_mut("global").map(|v| v.take()).unwrap_or_default();
    std::fs::create_dir_all(data_dir())?;
    write_json_atomic(&config_json(),   &serde_json::to_string_pretty(&global)?)?;
    write_json_atomic(&profiles_json(), &serde_json::to_string_pretty(&profiles)?)?;
    Ok(())
}

// ── Рабочие потоки ────────────────────────────────────────────────────────────
fn worker_install(hwnd: HWND) {
    let result = do_install(hwnd);
    let ok = result.is_ok();
    if let Err(e) = result {
        post_log(hwnd, format!("ОШИБКА: {:#}", e));
    }
    post_done(hwnd, ok);
}

fn do_install(hwnd: HWND) -> anyhow::Result<()> {
    use std::{fs, process::Command};
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    post_log(hwnd, "Подключение к серверу...".into());
    let manifest: serde_json::Value = client
        .get(&format!("{}/api/manifest", SERVER))
        .header("X-Api-Key", api_key())
        .send()?.error_for_status()?.json()?;

    let gui_ver   = manifest["gui_version"].as_str().unwrap_or("?").to_owned();
    let agent_ver = manifest["agent_version"].as_str().unwrap_or("?").to_owned();
    post_log(hwnd, format!("Сервер: GUI {}, агент {}", gui_ver, agent_ver));
    fs::create_dir_all(data_dir())?;
    harden_data_dir();

    post_log(hwnd, "Остановка службы...".into());
    let _ = Command::new("sc.exe").args(["stop","RestlexAgent"])
        .creation_flags(CREATE_NO_WINDOW).status();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = Command::new("taskkill").args(["/F","/IM","restlex.exe"])
        .creation_flags(CREATE_NO_WINDOW).status();
    let _ = Command::new("taskkill").args(["/F","/IM","restlex-agent.exe"])
        .creation_flags(CREATE_NO_WINDOW).status();
    std::thread::sleep(std::time::Duration::from_millis(500));

    post_log(hwnd, "Загрузка restlex.exe...".into());
    download_file(&client, "restlex.exe", &restlex_exe())?;
    post_log(hwnd, "  restlex.exe ОК".into());

    post_log(hwnd, "Загрузка restlex-agent.exe...".into());
    download_file(&client, "restlex-agent.exe", &agent_exe())?;
    post_log(hwnd, "  restlex-agent.exe ОК".into());

    let need_config = !config_json().exists();

    post_log(hwnd, "Установка службы RestlexAgent...".into());
    let st = Command::new(restlex_exe()).args(["install","--all"])
        .creation_flags(CREATE_NO_WINDOW).status()?;
    if !st.success() {
        anyhow::bail!("install --all завершился с кодом {:?}", st.code());
    }

    if need_config {
        post_log(hwnd, "Загрузка конфигурации...".into());
        let _ = Command::new("sc.exe").args(["stop","RestlexAgent"])
            .creation_flags(CREATE_NO_WINDOW).status();
        std::thread::sleep(std::time::Duration::from_secs(2));
        match apply_server_config(&client) {
            Ok(_) => {
                post_log(hwnd, "  config.json применён.".into());
                let _ = Command::new("sc.exe").args(["start","RestlexAgent"])
                    .creation_flags(CREATE_NO_WINDOW).status();
            }
            Err(e) => post_log(hwnd, format!("  ПРЕДУПРЕЖДЕНИЕ: конфиг: {:#}", e)),
        }
    } else {
        post_log(hwnd, "Конфиг уже есть, пропуск.".into());
    }

    // Ещё раз — уже после того, как config.json/profiles.json появились.
    harden_secret_files();

    let state = serde_json::json!({"gui_version": gui_ver, "agent_version": agent_ver});
    fs::write(state_file(), serde_json::to_string_pretty(&state)?)?;

    // Ярлык на рабочем столе текущего пользователя
    if let Ok(username) = std::env::var("USERNAME") {
        if !username.is_empty() {
            post_log(hwnd, format!("Создание ярлыка для {}...", username));
            create_user_shortcut(&username);
        }
    }

    post_log(hwnd, format!("Установка завершена! GUI {} / агент {}", gui_ver, agent_ver));
    Ok(())
}

/// Экранирование одинарных кавычек для PowerShell-строки: имя пользователя
/// приходит из окружения (%USERNAME%), и без удвоения `'` значение вида
/// `x';calc;#` выполняется как код в контексте установщика (админ).
fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

fn create_shortcut(lnk: &str) {
    let exe = restlex_exe().to_string_lossy().into_owned();
    let dir = data_dir().to_string_lossy().into_owned();
    let lnk = ps_quote(lnk);
    let exe = ps_quote(&exe);
    let dir = ps_quote(&dir);
    let ps = format!(
        "$s=(New-Object -COM WScript.Shell).CreateShortcut('{lnk}');\
         $s.TargetPath='{exe}';\
         $s.WorkingDirectory='{dir}';\
         $s.IconLocation='{exe},0';\
         $s.Save()"
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

fn create_user_shortcut(username: &str) {
    for desk in &["Desktop", "\u{0420}\u{0430}\u{0431}\u{043e}\u{0447}\u{0438}\u{0439} \u{0441}\u{0442}\u{043e}\u{043b}"] {
        let lnk = format!(r"C:\Users\{}\{}\Restlex.lnk", username, desk);
        if std::path::Path::new(&lnk).parent().map(|p| p.exists()).unwrap_or(false) {
            create_shortcut(&lnk);
            return;
        }
    }
}

fn worker_uninstall(hwnd: HWND) {
    let result = do_uninstall(hwnd);
    let ok = result.is_ok();
    if let Err(e) = result {
        post_log(hwnd, format!("ОШИБКА: {:#}", e));
    }
    post_done(hwnd, ok);
}

fn do_uninstall(hwnd: HWND) -> anyhow::Result<()> {
    use std::{fs, process::Command};
    if restlex_exe().exists() {
        post_log(hwnd, "Удаление службы RestlexAgent...".into());
        let _ = Command::new(restlex_exe()).args(["--uninstall-agent"])
            .creation_flags(CREATE_NO_WINDOW).status();
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    post_log(hwnd, "Удаление файлов...".into());
    for f in [restlex_exe(), agent_exe(), state_file()] {
        let _ = fs::remove_file(&f);
    }
    post_log(hwnd, "Удаление ярлыков...".into());
    let _ = fs::remove_file(r"C:\Users\Public\Desktop\Restlex.lnk");
    if let Ok(rd) = fs::read_dir(r"C:\Users") {
        for entry in rd.flatten() {
            for desk in ["Desktop", "Рабочий стол"] {
                let _ = fs::remove_file(entry.path().join(desk).join("Restlex.lnk"));
            }
        }
    }
    post_log(hwnd, "Удаление завершено.".into());
    Ok(())
}

// ── WndProc ───────────────────────────────────────────────────────────────────
extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                HWND_MAIN.store(hwnd as isize, Ordering::SeqCst);
                let hinst = GetModuleHandleW(ptr::null());

                // Заголовочные метки
                CreateWindowExW(0, w("STATIC").as_ptr(), w("RestLex — Установщик").as_ptr(),
                    WS_CHILD | WS_VISIBLE, 10, 8, 460, 22, hwnd, 0, hinst, ptr::null());
                CreateWindowExW(0, w("STATIC").as_ptr(), w(&format!("Сервер: {}", SERVER)).as_ptr(),
                    WS_CHILD | WS_VISIBLE, 10, 32, 460, 18, hwnd, 0, hinst, ptr::null());

                // Лог (многострочный edit, readonly)
                let h_log = CreateWindowExW(
                    WS_EX_CLIENTEDGE,
                    w("EDIT").as_ptr(), w("").as_ptr(),
                    WS_CHILD | WS_VISIBLE | WS_VSCROLL | ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL,
                    10, 58, 460, 216, hwnd, ID_LOG as isize, hinst, ptr::null(),
                );
                HWND_LOG.store(h_log as isize, Ordering::SeqCst);

                // Прогресс-бар
                let h_prog = CreateWindowExW(
                    0, w("msctls_progress32").as_ptr(), ptr::null(),
                    WS_CHILD | WS_VISIBLE,
                    10, 283, 460, 18, hwnd, ID_PROG as isize, hinst, ptr::null(),
                );
                HWND_PROG.store(h_prog as isize, Ordering::SeqCst);
                SendMessageW(h_prog, PBM_SETRANGE32, 0, 100);

                // Кнопка Установить
                let h_inst = CreateWindowExW(
                    0, w("BUTTON").as_ptr(), w("Установить").as_ptr(),
                    WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                    10, 311, 220, 36, hwnd, ID_INST as isize, hinst, ptr::null(),
                );
                HWND_INST.store(h_inst as isize, Ordering::SeqCst);

                // Кнопка Удалить
                let h_rem = CreateWindowExW(
                    0, w("BUTTON").as_ptr(), w("Удалить").as_ptr(),
                    WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                    250, 311, 220, 36, hwnd, ID_REM as isize, hinst, ptr::null(),
                );
                HWND_REM.store(h_rem as isize, Ordering::SeqCst);
                0
            }

            WM_COMMAND => {
                let id     = (wp & 0xFFFF) as usize;
                let notif  = ((wp >> 16) & 0xFFFF) as u32;
                if notif == BN_CLICKED as u32 {
                    if BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        let h_log  = HWND_LOG.load(Ordering::SeqCst) as HWND;
                        let h_prog = HWND_PROG.load(Ordering::SeqCst) as HWND;
                        // Очистить лог и прогресс
                        SendMessageW(h_log, EM_SETSEL, 0, -1isize);
                        SendMessageW(h_log, EM_REPLACESEL, 0, w("").as_ptr() as isize);
                        SendMessageW(h_prog, PBM_SETPOS, 0, 0);
                        // Отключить кнопки
                        EnableWindow(HWND_INST.load(Ordering::SeqCst) as HWND, 0);
                        EnableWindow(HWND_REM.load(Ordering::SeqCst) as HWND, 0);

                        let h = hwnd;
                        if id == ID_INST {
                            thread::spawn(move || worker_install(h));
                        } else if id == ID_REM {
                            thread::spawn(move || worker_uninstall(h));
                        } else {
                            BUSY.store(false, Ordering::SeqCst);
                            EnableWindow(HWND_INST.load(Ordering::SeqCst) as HWND, 1);
                            EnableWindow(HWND_REM.load(Ordering::SeqCst) as HWND, 1);
                        }
                    }
                }
                0
            }

            WM_WORKER_LOG => {
                let h_log = HWND_LOG.load(Ordering::SeqCst) as HWND;
                let lines: Vec<String> = {
                    let mut q = log_queue().lock().unwrap();
                    q.drain(..).collect()
                };
                for line in lines {
                    append_log(h_log, &line);
                }
                0
            }

            WM_WORKER_DONE => {
                BUSY.store(false, Ordering::SeqCst);
                EnableWindow(HWND_INST.load(Ordering::SeqCst) as HWND, 1);
                EnableWindow(HWND_REM.load(Ordering::SeqCst) as HWND, 1);
                SendMessageW(
                    HWND_PROG.load(Ordering::SeqCst) as HWND,
                    PBM_SETPOS,
                    if wp != 0 { 100 } else { 30 },
                    0,
                );
                0
            }

            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                BeginPaint(hwnd, &mut ps);
                EndPaint(hwnd, &ps);
                0
            }

            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }

            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

// ── Точка входа ───────────────────────────────────────────────────────────────
fn main() {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC:  ICC_PROGRESS_CLASS,
        };
        InitCommonControlsEx(&icc);

        let hinst = GetModuleHandleW(ptr::null());
        let class_name = w("RestLexKlik");

        let wc = WNDCLASSEXW {
            cbSize:        std::mem::size_of::<WNDCLASSEXW>() as u32,
            style:         CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc:   Some(wnd_proc),
            cbClsExtra:    0,
            cbWndExtra:    0,
            hInstance:     hinst,
            hIcon:         0,
            hCursor:       LoadCursorW(0, IDC_ARROW),
            hbrBackground: 16isize, // COLOR_BTNFACE + 1
            lpszMenuName:  ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm:       0,
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            w("RestLex — Установщик").as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT, CW_USEDEFAULT,
            495, 396,
            0, 0, hinst, ptr::null(),
        );

        ShowWindow(hwnd, SW_SHOWDEFAULT);
        UpdateWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

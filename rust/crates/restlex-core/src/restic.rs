use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackupSummary {
    pub snapshot_id: Option<String>,
    pub files_new: u64,
    pub files_changed: u64,
    pub bytes_added: u64,
    pub exit_code: i32,
    pub errors: Vec<String>,
    pub duration_s: u64,
}

/// Lines emitted by restic backup parsed into progress updates.
#[derive(Debug, Clone)]
pub enum Progress {
    /// PID запущенного restic — чтобы вызывающий мог прервать бэкап адресно,
    /// а не убивать все restic.exe на машине.
    Started(u32),
    Line(String),
    Stats {
        percent: f32,
        files: u64,
        total_bytes: u64,
    },
    Done(BackupSummary),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RestoreSummary {
    pub total_files: u64,
    pub files_restored: u64,
    pub files_skipped: u64,
    pub files_deleted: u64,
    pub total_bytes: u64,
    pub bytes_restored: u64,
    pub bytes_skipped: u64,
    pub seconds_elapsed: u64,
    pub exit_code: i32,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum RestoreProgress {
    /// PID процесса restic — чтобы вызывающий мог прервать restore адресно.
    Started(u32),
    Line(String),
    Status {
        percent: f32,
        files_restored: u64,
        total_files: u64,
        total_bytes: u64,
        bytes_restored: u64,
    },
    Done(RestoreSummary),
}

pub struct ResticRunner {
    pub restic_path: String,
    pub repo: String,
    pub password: String,
    pub password_file: Option<String>,
    pub temp: Option<String>,
    pub extra_env: std::collections::HashMap<String, String>,
    /// Тексты ошибок для людей (отчёты, ответы API) — по-английски.
    pub en: bool,
}

impl ResticRunner {
    pub fn new(
        restic_path: impl Into<String>,
        repo: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            restic_path: restic_path.into(),
            repo: repo.into(),
            password: password.into(),
            password_file: None,
            temp: None,
            extra_env: std::collections::HashMap::new(),
            en: false,
        }
    }

    /// Язык текстов ошибок из глобального config.json (`language`).
    pub fn with_language(mut self, lang: &str) -> Self {
        self.en = crate::i18n::is_en(lang);
        self
    }

    pub fn with_password_file(mut self, path: impl Into<String>) -> Self {
        let path = path.into();
        if !path.is_empty() {
            self.password_file = Some(path);
        }
        self
    }

    pub fn with_temp(mut self, temp: impl Into<String>) -> Self {
        let t = temp.into();
        if !t.is_empty() {
            self.temp = Some(t);
        }
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        let v = val.into();
        if !v.is_empty() {
            self.extra_env.insert(key.into(), v);
        }
        self
    }

    /// Общая заготовка команды. Здесь же единственная проверка пароля: если
    /// DPAPI не смог расшифровать `repo_password`, в `RESTIC_PASSWORD` ушёл бы
    /// `enc:`-шифроблоб. Проверка стоит именно тут, чтобы закрыть все пути
    /// (backup, restore, test, snapshots, forget, check, unlock, init, ls, dump).
    fn base_cmd(&self) -> Result<Command> {
        // Путь к бинарнику — тоже здесь, по той же причине: раньше проверка
        // стояла только в backup()/restore_with_progress(), и check_access
        // («Тест репозитория»), snapshots, version, init, unlock, check, forget,
        // ls, dump, repair index запускали ЧТО УГОДНО, включая
        // \\evil\share\x.exe, под LocalSystem.
        validate_restic_path_lang(&self.restic_path, self.en)?;
        let mut c = Command::new(&self.restic_path);
        c.env("RESTIC_REPOSITORY", &self.repo);
        // Глобальный --password-file применяем ТОЛЬКО когда у профиля нет своего
        // пароля: иначе все профили молча уходили на чужой пароль («wrong password»).
        match &self.password_file {
            Some(path) if self.password.is_empty() => {
                c.arg("--password-file").arg(path);
            }
            _ => {
                if crate::crypto::is_undecrypted(&self.password) {
                    return Err(Error::Other(
                        crate::i18n::tr(
                            self.en,
                            "repo_password не расшифрован (DPAPI): операция отменена",
                            "repo_password is not decrypted (DPAPI): operation cancelled",
                        )
                        .into(),
                    ));
                }
                c.env("RESTIC_PASSWORD", &self.password);
            }
        }
        if let Some(t) = &self.temp {
            c.env("TMP", t).env("TEMP", t);
        }
        for (k, v) in &self.extra_env {
            c.env(k, v);
        }
        c.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            // Иначе ошибка чтения строки/ранний выход оставляли restic.exe сиротой,
            // а он держит LOCK репозитория («repository is already locked»).
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            c.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        Ok(c)
    }

    pub async fn version(&self) -> Result<String> {
        let out = self.base_cmd()?.arg("version").output().await?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Query restic and report (raw version line, parsed semver, is-compatible).
    pub async fn version_info(&self) -> Result<(String, Option<(u32, u32, u32)>, bool)> {
        let raw = self.version().await?;
        let parsed = parse_restic_version(&raw);
        let ok = parsed.map(version_compatible).unwrap_or(false);
        Ok((raw, parsed, ok))
    }

    pub async fn init(&self) -> Result<String> {
        let out = self.base_cmd()?.arg("init").output().await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into())
    }

    pub async fn unlock(&self) -> Result<String> {
        let out = self.base_cmd()?.arg("unlock").output().await?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub async fn repair_index(&self) -> Result<String> {
        let out = self.base_cmd()?.args(["repair", "index"]).output().await?;
        Ok(mask_repo_creds(&format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }

    pub async fn check(&self) -> Result<()> {
        let out = self.base_cmd()?.arg("check").output().await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(())
    }

    pub async fn snapshots_json(&self) -> Result<String> {
        let out = self
            .base_cmd()?
            .arg("snapshots")
            .arg("--json")
            .output()
            .await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into())
    }

    /// `restic ls --json <snap> [path]` -- NDJSON-строки узлов. С `path` без
    /// `recursive` читается только содержимое этой папки (быстро и по сети);
    /// пустой `path` + `recursive` -- весь снимок.
    pub async fn ls_json_ex(&self, snap: &str, path: &str, recursive: bool) -> Result<String> {
        let mut cmd = self.base_cmd()?;
        cmd.arg("ls").arg("--json");
        if recursive && !path.is_empty() {
            cmd.arg("--recursive");
        }
        cmd.arg(snap);
        if !path.is_empty() {
            cmd.arg(path);
        }
        let out = cmd.output().await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into())
    }
}

/// `rest:http://user:pass@host/repo` -> `rest:http://host/repo`.
/// restic печатает строку репозитория в stderr («unable to open repository at …»),
/// а у REST-репозитория креды зашиты прямо в URL — наружу это отдавать нельзя.
pub fn mask_repo_creds(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("//") {
        let (head, tail) = rest.split_at(i + 2);
        out.push_str(head);
        // userinfo — всё до последнего '@' перед первым разделителем пути:
        // пароль может содержать сырой '@'.
        let end = tail.find(['/', ' ', '"', '\'', ')']).unwrap_or(tail.len());
        match tail[..end].rfind('@') {
            Some(at) => {
                rest = &tail[at + 1..];
            }
            None => {
                out.push_str(&tail[..end]);
                rest = &tail[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Опции restic, позволяющие выполнить произвольную команду. `extra_args`
/// приходят из `sched.params` (в т.ч. через REST API v1), а служба работает под
/// LocalSystem — без фильтра это RCE под SYSTEM.
///
/// Шелл-метасимволы (`&`, `|`) НЕ фильтруем: аргументы уходят через
/// `CreateProcess` без шелла, инъекции через них нет, а вот легальный путь вроде
/// `D:\R&D\ex.txt` они бы отбросили.
pub fn is_dangerous_arg(arg: &str) -> bool {
    let a = arg.trim().to_ascii_lowercase();
    if a.contains('\n') || a.contains('\r') {
        return true;
    }
    // --password-command, --stdin-from-command, -o sftp.command, -o sftp.args,
    // -o rclone.program, -o rclone.args …
    // Только опции: значение (путь/тег) может содержать эти подстроки легально.
    // Поэтому: (1) суффикс имени смотрим лишь у элементов, начинающихся с '-'
    // — иначе `--tag pre-command` или путь `D:\x.command` после `--exclude`
    // роняли ВЕСЬ бэкап профиля; (2) `-o key=value` — имя это часть до '='.
    let name = a.split('=').next().unwrap_or("");
    let is_option = a.starts_with('-');
    (is_option
        && (name.ends_with("-command")
            || name.ends_with(".command")
            || name.ends_with(".args")
            || name.ends_with(".program")))
        || a.contains("rclone.program")
        || a.contains("rclone.args")
        || a.contains("sftp.command")
        || a.contains("sftp.args")
}

/// Путь к restic.exe. UNC (`\\server\share`) запрещён — иначе через конфиг/REST
/// API подсовывается чужой бинарь под SYSTEM. Голое имя без разделителя пути
/// (`restic`, `restic.exe`) пропускаем как есть: и агент (`scheduler.rs`,
/// `main.rs`), и GUI штатно подставляют его в расчёте на резолв через `PATH`.
/// Существование проверяем только для путей — у службы под LocalSystem CWD
/// `C:\Windows\system32`, и `Path::new("restic").is_file()` там всегда false.
pub fn validate_restic_path(path: &str) -> Result<()> {
    validate_restic_path_lang(path, false)
}

/// [`validate_restic_path`] с языком текста ошибки (`en` — английский).
pub fn validate_restic_path_lang(path: &str, en: bool) -> Result<()> {
    let p = path.trim();
    if p.is_empty() {
        return Err(Error::Other(
            crate::i18n::tr(en, "путь к restic не задан", "restic path is not set").into(),
        ));
    }
    if p.starts_with("\\\\") || p.starts_with("//") {
        return Err(Error::Other(if en {
            format!("UNC path to restic is not allowed: {p}")
        } else {
            format!("путь к restic по UNC запрещён: {p}")
        }));
    }
    if !p.contains('\\') && !p.contains('/') {
        return Ok(());
    }
    if !std::path::Path::new(p).is_file() {
        return Err(Error::Other(if en {
            format!("restic not found: {p}")
        } else {
            format!("restic не найден: {p}")
        }));
    }
    Ok(())
}

impl ResticRunner {
    /// Проверка доступности репозитория: сеть, учётка и пароль сразу.
    /// `cat config` дешевле `snapshots` — читает один маленький файл.
    pub async fn check_access(&self) -> Result<()> {
        let out = self
            .base_cmd()?
            .arg("cat")
            .arg("config")
            .output()
            .await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(())
    }

    pub async fn ls_json(&self, snap: &str, path: Option<&str>) -> Result<String> {
        let mut c = self.base_cmd()?;
        c.arg("ls").arg("--json").arg(snap);
        if let Some(p) = path {
            c.arg(p);
        }
        let out = c.output().await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into())
    }

    pub async fn dump_bytes(&self, snap: &str, path: &str) -> Result<Vec<u8>> {
        let out = self
            .base_cmd()?
            .arg("dump")
            .arg(snap)
            .arg(path)
            .output()
            .await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(out.stdout)
    }

    pub async fn forget(&self, retention: &crate::Retention, prune: bool) -> Result<String> {
        let mut c = self.base_cmd()?;
        c.arg("forget");
        if prune {
            c.arg("--prune");
        }
        let r = retention;
        if r.keep_last > 0 {
            c.arg(format!("--keep-last={}", r.keep_last));
        }
        if r.keep_hourly > 0 {
            c.arg(format!("--keep-hourly={}", r.keep_hourly));
        }
        if r.keep_daily > 0 {
            c.arg(format!("--keep-daily={}", r.keep_daily));
        }
        if r.keep_weekly > 0 {
            c.arg(format!("--keep-weekly={}", r.keep_weekly));
        }
        if r.keep_monthly > 0 {
            c.arg(format!("--keep-monthly={}", r.keep_monthly));
        }
        if r.keep_yearly > 0 {
            c.arg(format!("--keep-yearly={}", r.keep_yearly));
        }
        let out = c.output().await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into())
    }

    pub async fn restore(&self, snap: &str, target: &str, includes: &[String]) -> Result<()> {
        let mut c = self.base_cmd()?;
        c.arg("restore").arg(snap).arg("--target").arg(target);
        for include in includes {
            if !include.trim().is_empty() {
                c.arg("--include").arg(include);
            }
        }
        let out = c.output().await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(())
    }

    pub async fn restore_with_progress<F>(
        &self,
        snap: &str,
        target: &str,
        includes: &[String],
        mut on_progress: F,
    ) -> Result<RestoreSummary>
    where
        F: FnMut(RestoreProgress) + Send,
    {
        // validate_restic_path — в base_cmd(): закрывает все пути сразу.
        let mut c = self.base_cmd()?;
        c.arg("restore")
            .arg(snap)
            .arg("--target")
            .arg(target)
            .arg("--json")
            .arg("--verbose=2");
        for include in includes {
            if !include.trim().is_empty() {
                c.arg("--include").arg(include);
            }
        }

        let mut child = c.spawn()?;
        if let Some(id) = child.id() {
            on_progress(RestoreProgress::Started(id));
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Other("no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Other("no stderr".into()))?;

        let mut summary = RestoreSummary::default();

        // Drain stderr concurrently to avoid a stdout/stderr pipe deadlock.
        let (etx, mut erx) = tokio::sync::mpsc::channel::<String>(256);
        let stderr_task = tokio::spawn(async move {
            let mut erdr = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = erdr.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                if etx.send(line).await.is_err() {
                    break;
                }
            }
        });

        let mut err_lines = Vec::new();
        let mut rdr = BufReader::new(stdout).lines();
        let mut stdout_done = false;
        let mut stderr_done = false;
        while !(stdout_done && stderr_done) {
            tokio::select! {
                line = rdr.next_line(), if !stdout_done => {
                    // Ошибку декодирования строки (имя файла в cp1251) НЕ пробрасываем:
                    // выход по `?` оставлял бы restic сиротой с локом на репозитории.
                    // Любая ДРУГАЯ ошибка чтения пайпа устойчива: `continue` крутил бы
                    // select! на 100% CPU и никогда не дошёл бы до child.wait().
                    let line = match line {
                        Ok(v) => v,
                        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                            tracing::warn!("restic stdout: строка не декодирована: {e}");
                            continue;
                        }
                        Err(e) => {
                            tracing::error!("restic stdout: чтение прервано: {e}");
                            stdout_done = true;
                            continue;
                        }
                    };
                    match line {
                        None => stdout_done = true,
                        Some(line) => {
                            if line.trim().is_empty() {
                                continue;
                            }
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                                match v.get("message_type").and_then(|x| x.as_str()) {
                                    Some("status") => {
                                        on_progress(RestoreProgress::Status {
                                            percent: v
                                                .get("percent_done")
                                                .and_then(|x| x.as_f64())
                                                .unwrap_or(0.0) as f32,
                                            files_restored: v
                                                .get("files_restored")
                                                .and_then(|x| x.as_u64())
                                                .unwrap_or(0),
                                            total_files: v.get("total_files").and_then(|x| x.as_u64()).unwrap_or(0),
                                            total_bytes: v.get("total_bytes").and_then(|x| x.as_u64()).unwrap_or(0),
                                            bytes_restored: v
                                                .get("bytes_restored")
                                                .and_then(|x| x.as_u64())
                                                .unwrap_or(0),
                                        });
                                    }
                                    Some("verbose_status") => {
                                        let action = v
                                            .get("action")
                                            .and_then(|x| x.as_str())
                                            .unwrap_or("restored");
                                        let item = v.get("item").and_then(|x| x.as_str()).unwrap_or("");
                                        on_progress(RestoreProgress::Line(format!("{action}: {item}")));
                                    }
                                    Some("summary") => {
                                        summary.total_files =
                                            v.get("total_files").and_then(|x| x.as_u64()).unwrap_or(0);
                                        summary.files_restored = v
                                            .get("files_restored")
                                            .and_then(|x| x.as_u64())
                                            .unwrap_or(0);
                                        summary.files_skipped =
                                            v.get("files_skipped").and_then(|x| x.as_u64()).unwrap_or(0);
                                        summary.files_deleted =
                                            v.get("files_deleted").and_then(|x| x.as_u64()).unwrap_or(0);
                                        summary.total_bytes =
                                            v.get("total_bytes").and_then(|x| x.as_u64()).unwrap_or(0);
                                        summary.bytes_restored = v
                                            .get("bytes_restored")
                                            .and_then(|x| x.as_u64())
                                            .unwrap_or(0);
                                        summary.bytes_skipped =
                                            v.get("bytes_skipped").and_then(|x| x.as_u64()).unwrap_or(0);
                                        summary.seconds_elapsed = v
                                            .get("seconds_elapsed")
                                            .and_then(|x| x.as_u64())
                                            .unwrap_or(0);
                                    }
                                    _ => {} // unknown JSON message type — skip silently
                                }
                            } else {
                                on_progress(RestoreProgress::Line(line.clone()));
                            }
                        }
                    }
                }
                msg = erx.recv(), if !stderr_done => {
                    match msg {
                        None => stderr_done = true,
                        Some(line) => {
                            let msg = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                                let item = v.get("item").and_then(|x| x.as_str()).unwrap_or("");
                                let err = v
                                    .get("error")
                                    .and_then(|x| x.get("message"))
                                    .and_then(|x| x.as_str())
                                    .unwrap_or(line.as_str());
                                if item.is_empty() {
                                    format!("ERR: {err}")
                                } else {
                                    format!("ERR: {item} — {err}")
                                }
                            } else {
                                format!("ERR: {line}")
                            };
                            let msg = mask_repo_creds(&msg);
                            err_lines.push(msg.clone());
                            on_progress(RestoreProgress::Line(msg));
                        }
                    }
                }
            }
        }
        if let Err(e) = stderr_task.await {
            tracing::error!("сбор stderr restic упал: {e}");
        }
        summary.errors = err_lines;

        let status = child.wait().await?;
        summary.exit_code = status.code().unwrap_or(-1);
        on_progress(RestoreProgress::Done(summary.clone()));

        if !status.success() {
            let msg = if summary.errors.is_empty() {
                format!("restore failed with exit code {}", summary.exit_code)
            } else {
                summary.errors.join("\n")
            };
            return Err(Error::Restic(msg));
        }
        Ok(summary)
    }

    pub async fn forget_snapshot(&self, snap: &str, prune: bool) -> Result<()> {
        let mut c = self.base_cmd()?;
        c.arg("forget").arg(snap);
        if prune {
            c.arg("--prune");
        }
        let out = c.output().await?;
        if !out.status.success() {
            return Err(Error::Restic(mask_repo_creds(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(())
    }

    /// Run backup and stream progress via callback.
    pub async fn backup<F>(
        &self,
        sources: &[String],
        use_vss: bool,
        excludes: &[String],
        no_scan: bool,
        vss_timeout_s: u64,
        extra_args: &[String],
        mut on_progress: F,
    ) -> Result<BackupSummary>
    where
        F: FnMut(Progress) + Send,
    {
        let started = std::time::Instant::now();
        // Проверки пути к restic и нерасшифрованного repo_password — в
        // base_cmd(): закрывают все пути сразу, не только backup.
        let mut c = self.base_cmd()?;
        c.arg("backup").arg("--json");
        if use_vss {
            c.arg("--use-fs-snapshot");
            if vss_timeout_s > 0 {
                c.arg("-o").arg(format!("vss.timeout={vss_timeout_s}s"));
            }
        }
        if no_scan {
            c.arg("--no-scan");
        }
        for e in excludes {
            c.arg("--exclude").arg(e);
        }
        for arg in extra_args {
            if arg.trim().is_empty() {
                continue;
            }
            // Прерываем, а не пропускаем: extra_args — плоский список, где опция и
            // её значение лежат двумя соседними элементами. Выброшенный элемент
            // оставил бы опцию без значения, и restic съел бы как значение первый
            // путь-источник — бэкап снял бы не тот набор данных.
            if is_dangerous_arg(arg) {
                return Err(Error::Other(if self.en {
                    format!("extra_args: argument rejected as dangerous: {arg}")
                } else {
                    format!("extra_args: аргумент отклонён как опасный: {arg}")
                }));
            }
            c.arg(arg);
        }
        for s in sources {
            c.arg(s);
        }

        let mut child = c.spawn()?;
        if let Some(id) = child.id() {
            on_progress(Progress::Started(id));
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Other("no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Other("no stderr".into()))?;

        let mut summary = BackupSummary::default();

        // Drain stderr concurrently. Reading stdout to completion *before*
        // touching stderr can deadlock: if restic fills its stderr pipe buffer
        // (e.g. many VSS warnings) it blocks on write while we block on stdout.
        let (etx, mut erx) = tokio::sync::mpsc::channel::<String>(256);
        let stderr_task = tokio::spawn(async move {
            let mut errs = Vec::new();
            let mut erdr = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = erdr.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let line = mask_repo_creds(&line);
                errs.push(line.clone());
                if etx.send(line).await.is_err() {
                    break;
                }
            }
            errs
        });

        // Parse stdout (JSON messages)
        let mut rdr = BufReader::new(stdout).lines();
        let mut stdout_done = false;
        let mut stderr_done = false;
        while !(stdout_done && stderr_done) {
            tokio::select! {
                line = rdr.next_line(), if !stdout_done => {
                    // Ошибку декодирования строки (имя файла в cp1251) НЕ пробрасываем:
                    // выход по `?` оставлял бы restic сиротой с локом на репозитории.
                    // Любая ДРУГАЯ ошибка чтения пайпа устойчива: `continue` крутил бы
                    // select! на 100% CPU и никогда не дошёл бы до child.wait().
                    let line = match line {
                        Ok(v) => v,
                        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                            tracing::warn!("restic stdout: строка не декодирована: {e}");
                            continue;
                        }
                        Err(e) => {
                            tracing::error!("restic stdout: чтение прервано: {e}");
                            stdout_done = true;
                            continue;
                        }
                    };
                    match line {
                        None => stdout_done = true,
                        Some(line) => {
                            if line.trim().is_empty() {
                                continue;
                            }
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                                match v.get("message_type").and_then(|x| x.as_str()) {
                                    Some("status") => {
                                        let percent = v
                                            .get("percent_done")
                                            .and_then(|x| x.as_f64())
                                            .unwrap_or(0.0) as f32;
                                        let files = v.get("files_done").and_then(|x| x.as_u64()).unwrap_or(0);
                                        let bytes = v.get("total_bytes").and_then(|x| x.as_u64()).unwrap_or(0);
                                        on_progress(Progress::Stats {
                                            percent,
                                            files,
                                            total_bytes: bytes,
                                        });
                                    }
                                    Some("summary") => {
                                        summary.snapshot_id = v
                                            .get("snapshot_id")
                                            .and_then(|x| x.as_str())
                                            .map(String::from);
                                        summary.files_new =
                                            v.get("files_new").and_then(|x| x.as_u64()).unwrap_or(0);
                                        summary.files_changed =
                                            v.get("files_changed").and_then(|x| x.as_u64()).unwrap_or(0);
                                        summary.bytes_added =
                                            v.get("data_added").and_then(|x| x.as_u64()).unwrap_or(0);
                                    }
                                    _ => {} // unknown JSON message type — skip silently
                                }
                            } else {
                                on_progress(Progress::Line(line.clone()));
                            }
                        }
                    }
                }
                msg = erx.recv(), if !stderr_done => {
                    match msg {
                        None => stderr_done = true,
                        Some(line) => on_progress(Progress::Line(format!("ERR: {}", line))),
                    }
                }
            }
        }

        summary.errors = match stderr_task.await {
            Ok(errs) => errs,
            Err(e) => {
                tracing::error!("сбор stderr restic упал: {e}");
                vec![if self.en {
                    format!("ERR: collecting restic stderr failed: {e}")
                } else {
                    format!("ERR: сбор stderr restic упал: {e}")
                }]
            }
        };

        let status = child.wait().await?;
        summary.exit_code = status.code().unwrap_or(-1);
        summary.duration_s = started.elapsed().as_secs();
        on_progress(Progress::Done(summary.clone()));
        Ok(summary)
    }
}

/// Minimum restic version we support (`--use-fs-snapshot` JSON, modern restore
/// progress all require >= 0.16).
pub const MIN_RESTIC_VERSION: (u32, u32, u32) = (0, 16, 0);

/// Extract `(major, minor, patch)` from a `restic version` line such as
/// `"restic 0.16.4 compiled with go1.21.6 on windows/amd64"`.
pub fn parse_restic_version(s: &str) -> Option<(u32, u32, u32)> {
    let token = s.split_whitespace().find(|t| {
        let t = t.trim_start_matches('v');
        t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('.')
    })?;
    let token = token.trim_start_matches('v');
    let mut it = token.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it
        .next()
        .map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    Some((major, minor, patch))
}

/// True when `parsed` meets [`MIN_RESTIC_VERSION`].
pub fn version_compatible(parsed: (u32, u32, u32)) -> bool {
    parsed >= MIN_RESTIC_VERSION
}

#[cfg(test)]
mod mask_tests {
    use super::mask_repo_creds;

    #[test]
    fn strips_userinfo_from_repo_urls() {
        assert_eq!(
            mask_repo_creds("Fatal: unable to open repository at rest:http://lex:s3cret@10.0.0.1:8001/repo: 401"),
            "Fatal: unable to open repository at rest:http://10.0.0.1:8001/repo: 401"
        );
        // сырой '@' в пароле: режем до последнего разделителя, а не до первого
        let out = mask_repo_creds("at rest:http://u:p@ss@host:8001/r failed");
        assert_eq!(out, "at rest:http://host:8001/r failed");
        assert!(!out.contains("ss@"), "{out}");
        // без userinfo — текст не меняется
        assert_eq!(mask_repo_creds("at rest:http://host:8001/repo"), "at rest:http://host:8001/repo");
        assert_eq!(mask_repo_creds("no url here"), "no url here");
        assert_eq!(mask_repo_creds(""), "");
        // локальный путь с '@' в имени не должен пострадать
        assert_eq!(mask_repo_creds(r"C:\backup\a@b"), r"C:\backup\a@b");
        // несколько URL в одной строке
        assert_eq!(
            mask_repo_creds("a rest:http://u:p@h1/r b rest:http://u2:p2@h2/r"),
            "a rest:http://h1/r b rest:http://h2/r"
        );
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn dangerous_extra_args_are_rejected() {
        assert!(is_dangerous_arg("--password-command=cmd /c calc"));
        assert!(is_dangerous_arg("--PASSWORD-COMMAND=x"));
        assert!(is_dangerous_arg("--stdin-from-command"));
        assert!(is_dangerous_arg("-o sftp.command=evil"));
        assert!(is_dangerous_arg("sftp.command=evil"));
        assert!(is_dangerous_arg("-o rclone.program=cmd"));
        assert!(is_dangerous_arg("-o rclone.args=x"));
        // sftp.args уезжает аргументами в ssh: -o ProxyCommand=calc = RCE под SYSTEM.
        assert!(is_dangerous_arg("-o sftp.args=-oProxyCommand=calc"));
        assert!(is_dangerous_arg("sftp.args=-oProxyCommand=calc"));
        assert!(is_dangerous_arg("--tag=a\ncalc"));
        // нормальные аргументы проходят
        assert!(!is_dangerous_arg("--tag=nightly"));
        assert!(!is_dangerous_arg("--exclude-caches"));
        assert!(!is_dangerous_arg(r"--cache-dir=C:\restic-cache"));
        assert!(!is_dangerous_arg("--one-file-system"));
    }

    /// Ложные срабатывания: extra_args — плоский список «опция, значение», и
    /// любое отклонение теперь роняет бэкап целиком, поэтому шелл-метасимволы
    /// в путях обязаны проходить.
    #[test]
    fn ordinary_options_and_paths_are_not_dangerous() {
        for a in [
            "--exclude",
            "--exclude-file",
            "--iexclude",
            "--tag",
            "--host",
            "--one-file-system",
            "--limit-upload",
            "--read-concurrency",
            r"D:\R&D\ex.txt",
            r"D:\Отчёты и планы\1С|копия",
            r"C:\Program Files\1C\base",
            "8",
            // Значения после --tag/--exclude: суффикс имени проверяем ТОЛЬКО у
            // элементов, начинающихся с '-'. Иначе весь бэкап профиля падал бы
            // с Err на легальном теге или пути.
            "pre-command",
            "post-command",
            r"D:\backup\x.command",
        ] {
            assert!(!is_dangerous_arg(a), "ложное срабатывание на {a:?}");
        }
    }

    /// `base_cmd` — единственная точка, через которую собираются ВСЕ команды
    /// restic. UNC-путь обязан отваливаться и здесь, а не только в backup().
    #[test]
    fn base_cmd_rejects_unc_restic_path() {
        let r = ResticRunner::new(r"\\evil\share\x.exe", "repo", "pw");
        assert!(r.base_cmd().is_err(), "UNC-путь к restic должен отвергаться");
        let r2 = ResticRunner::new("//evil/share/x.exe", "repo", "pw");
        assert!(r2.base_cmd().is_err());
        // Голое имя резолвится через PATH — проходит.
        assert!(ResticRunner::new("restic.exe", "repo", "pw").base_cmd().is_ok());
        // Пустой путь — тоже отказ.
        assert!(ResticRunner::new("", "repo", "pw").base_cmd().is_err());
    }

    #[test]
    fn bare_restic_name_resolves_via_path() {
        // Служба под LocalSystem имеет CWD C:\Windows\system32 — голое имя
        // обязано пройти без проверки существования, его резолвит ОС через PATH.
        assert!(validate_restic_path("restic").is_ok());
        assert!(validate_restic_path("restic.exe").is_ok());
        assert!(validate_restic_path("  restic  ").is_ok());
    }

    #[test]
    fn restic_path_must_be_local_and_exist() {
        assert!(validate_restic_path("").is_err());
        assert!(validate_restic_path(r"\\evil\share\x.exe").is_err());
        assert!(validate_restic_path("//evil/share/x.exe").is_err());
        assert!(validate_restic_path(r"C:\nope\restic-does-not-exist.exe").is_err());
        // существующий абсолютный путь — ок
        let me = std::env::current_exe().unwrap();
        assert!(validate_restic_path(me.to_str().unwrap()).is_ok());
    }
}

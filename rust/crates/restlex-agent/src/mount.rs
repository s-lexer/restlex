use anyhow::{anyhow, Context};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{
        header::{
            ALLOW, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HOST, LAST_MODIFIED,
            USER_AGENT,
        },
        HeaderMap, HeaderValue, Method, StatusCode, Uri,
    },
    response::Response,
    routing::any,
    Router,
};
use chrono::{DateTime, Local, Utc};
use std::sync::Mutex;
use restlex_core::{restic::ResticRunner, Profile};
use serde::Serialize;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::oneshot;

#[derive(Clone, Debug, Default, Serialize)]
pub struct MountStatus {
    pub active: bool,
    pub pid: Option<String>,
    pub profile_name: Option<String>,
    pub port: Option<u16>,
    pub unc_path: Option<String>,
    pub started_at: Option<String>,
    pub note: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

pub struct MountSession {
    pub info: MountStatus,
    pub shutdown: Option<oneshot::Sender<()>>,
    pub handle: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct SnapshotInfo {
    id: String,
    short_id: String,
    dir_name: String,
    time: DateTime<Local>,
}

#[derive(Clone)]
struct DavNode {
    path: String,
    name: String,
    is_dir: bool,
    size: u64,
    modified: DateTime<Local>,
}

#[derive(Default)]
struct SnapshotTree {
    nodes: HashMap<String, DavNode>,
    children: HashMap<String, Vec<String>>,
}

#[derive(Clone)]
struct VfsEntry {
    href_path: String,
    name: String,
    is_dir: bool,
    size: u64,
    modified: DateTime<Local>,
}

struct MountFs {
    pid: String,
    profile_name: String,
    runner: ResticRunner,
    snapshots: Vec<SnapshotInfo>,
    latest: Option<SnapshotInfo>,
    trees: Mutex<HashMap<String, Arc<SnapshotTree>>>,
}

// Basic-авторизации здесь СОЗНАТЕЛЬНО нет. Windows WebClient по умолчанию имеет
// BasicAuthLevel=1 («Basic только поверх SSL») и по plain HTTP креды не отдаёт:
// `net use \\localhost@port\DavWWWRoot` получал бы 401 и падал с 1244 у всех.
// Защита — bind на 127.0.0.1 (см. start): слушатель недоступен из сети, а всё,
// что крутится локально под тем же пользователем, и так имеет доступ к репо.

impl SnapshotTree {
    fn add_node(&mut self, node: DavNode) {
        let parent = parent_path(&node.path);
        self.children
            .entry(parent)
            .or_default()
            .push(node.path.clone());
        self.nodes.insert(node.path.clone(), node);
    }

    fn sort_children(&mut self) {
        for children in self.children.values_mut() {
            children.sort_by(|a, b| {
                let left = self.nodes.get(a);
                let right = self.nodes.get(b);
                match (left, right) {
                    (Some(l), Some(r)) => (u8::from(!l.is_dir), l.name.to_lowercase())
                        .cmp(&(u8::from(!r.is_dir), r.name.to_lowercase())),
                    _ => a.cmp(b),
                }
            });
        }
    }
}

impl MountFs {
    async fn new(
        cfg: &restlex_core::Config,
        pid: String,
        profile: &Profile,
    ) -> anyhow::Result<Self> {
        let runner = crate::scheduler::build_runner(cfg, profile);
        let raw = runner.snapshots_json().await?;
        let mut snaps = parse_snapshots(&raw, runner.en)?;
        snaps.sort_by(|a, b| b.time.cmp(&a.time));
        for (idx, snap) in snaps.iter_mut().enumerate() {
            snap.dir_name = format!(
                "{:04}__{}__{}",
                idx + 1,
                snap.time.format("%Y-%m-%d_%H-%M-%S"),
                snap.short_id
            );
        }
        let latest = snaps.first().cloned();
        Ok(Self {
            pid,
            profile_name: profile.name.clone(),
            runner,
            snapshots: snaps,
            latest,
            trees: Mutex::new(HashMap::new()),
        })
    }

    fn latest_snapshot(&self) -> Option<SnapshotInfo> {
        self.latest.clone()
    }

    fn snapshot_by_dir_name(&self, dir_name: &str) -> Option<SnapshotInfo> {
        self.snapshots
            .iter()
            .find(|snap| snap.dir_name == dir_name)
            .cloned()
    }

    async fn load_tree(&self, snap: &SnapshotInfo) -> anyhow::Result<Arc<SnapshotTree>> {
        if let Some(tree) = self.trees.lock().unwrap().get(&snap.id).cloned() {
            return Ok(tree);
        }

        let raw = self.runner.ls_json(&snap.id, None).await?;
        let mut tree = SnapshotTree::default();
        tree.nodes.insert(
            "/".to_string(),
            DavNode {
                path: "/".to_string(),
                name: String::new(),
                is_dir: true,
                size: 0,
                modified: snap.time,
            },
        );
        tree.children.entry("/".to_string()).or_default();

        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: serde_json::Value = match serde_json::from_str(line) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if value.get("struct_type").and_then(|v| v.as_str()) != Some("node") {
                continue;
            }

            let path = normalize_snap_path(value.get("path").and_then(|v| v.as_str()).unwrap_or("/"));
            if path == "/" {
                continue;
            }

            let node = DavNode {
                name: value
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| file_name(&path)),
                path: path.clone(),
                is_dir: value.get("type").and_then(|v| v.as_str()) == Some("dir"),
                size: value.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                modified: parse_dt(
                    value.get("mtime").and_then(|v| v.as_str()),
                    snap.time,
                ),
            };
            tree.add_node(node);
        }

        tree.sort_children();
        let tree = Arc::new(tree);
        self.trees
            .lock()
            .unwrap()
            .insert(snap.id.clone(), tree.clone());
        Ok(tree)
    }

    async fn stat_path(&self, path: &str) -> anyhow::Result<VfsEntry> {
        let path = normalize_http_path(path);
        if path == "/" {
            return Ok(VfsEntry {
                href_path: "/".to_string(),
                name: "DavWWWRoot".to_string(),
                is_dir: true,
                size: 0,
                modified: self
                    .latest_snapshot()
                    .map(|snap| snap.time)
                    .unwrap_or_else(Local::now),
            });
        }

        if path == "/snapshots" {
            return Ok(VfsEntry {
                href_path: path,
                name: "snapshots".to_string(),
                is_dir: true,
                size: 0,
                modified: self
                    .latest_snapshot()
                    .map(|snap| snap.time)
                    .unwrap_or_else(Local::now),
            });
        }

        if path == "/latest" {
            let snap = self.latest_snapshot().ok_or_else(|| anyhow!("Снапшоты не найдены"))?;
            return Ok(VfsEntry {
                href_path: path,
                name: "latest".to_string(),
                is_dir: true,
                size: 0,
                modified: snap.time,
            });
        }

        let (snap, prefix, rel_path) = self.resolve_snapshot_path(&path)?;
        if rel_path == "/" {
            return Ok(VfsEntry {
                href_path: path,
                name: prefix
                    .rsplit('/')
                    .next()
                    .unwrap_or("snapshot")
                    .to_string(),
                is_dir: true,
                size: 0,
                modified: snap.time,
            });
        }

        let tree = self.load_tree(&snap).await?;
        let node = tree
            .nodes
            .get(&rel_path)
            .cloned()
            .ok_or_else(|| anyhow!("Путь не найден: {rel_path}"))?;
        Ok(VfsEntry {
            href_path: path,
            name: node.name,
            is_dir: node.is_dir,
            size: node.size,
            modified: node.modified,
        })
    }

    async fn list_dir(&self, path: &str) -> anyhow::Result<Vec<VfsEntry>> {
        let path = normalize_http_path(path);
        if path == "/" {
            let mut items = vec![VfsEntry {
                href_path: "/snapshots".to_string(),
                name: "snapshots".to_string(),
                is_dir: true,
                size: 0,
                modified: self
                    .latest_snapshot()
                    .map(|snap| snap.time)
                    .unwrap_or_else(Local::now),
            }];
            if let Some(snap) = self.latest_snapshot() {
                items.insert(
                    0,
                    VfsEntry {
                        href_path: "/latest".to_string(),
                        name: "latest".to_string(),
                        is_dir: true,
                        size: 0,
                        modified: snap.time,
                    },
                );
            }
            return Ok(items);
        }

        if path == "/snapshots" {
            return Ok(self
                .snapshots
                .iter()
                .map(|snap| VfsEntry {
                    href_path: format!("/snapshots/{}", snap.dir_name),
                    name: snap.dir_name.clone(),
                    is_dir: true,
                    size: 0,
                    modified: snap.time,
                })
                .collect());
        }

        let (snap, prefix, rel_path) = self.resolve_snapshot_path(&path)?;
        let tree = self.load_tree(&snap).await?;
        let child_paths = tree
            .children
            .get(&rel_path)
            .cloned()
            .unwrap_or_default();
        let mut items = Vec::with_capacity(child_paths.len());
        for child_path in child_paths {
            if let Some(node) = tree.nodes.get(&child_path) {
                items.push(VfsEntry {
                    href_path: format!("{prefix}{child_path}"),
                    name: node.name.clone(),
                    is_dir: node.is_dir,
                    size: node.size,
                    modified: node.modified,
                });
            }
        }
        Ok(items)
    }

    async fn read_file(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let path = normalize_http_path(path);
        let (snap, _, rel_path) = self.resolve_snapshot_path(&path)?;
        if rel_path == "/" {
            return Err(anyhow!("Каталог нельзя читать как файл"));
        }

        let tree = self.load_tree(&snap).await?;
        let node = tree
            .nodes
            .get(&rel_path)
            .ok_or_else(|| anyhow!("Файл не найден"))?;
        if node.is_dir {
            return Err(anyhow!("Каталог нельзя читать как файл"));
        }
        Ok(self.runner.dump_bytes(&snap.id, &rel_path).await?)
    }

    fn resolve_snapshot_path(&self, path: &str) -> anyhow::Result<(SnapshotInfo, String, String)> {
        let parts: Vec<&str> = path
            .trim_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();

        match parts.as_slice() {
            ["latest"] | ["latest", ..] => {
                let snap = self.latest_snapshot().ok_or_else(|| anyhow!("Снапшоты не найдены"))?;
                let rel_path = if parts.len() > 1 {
                    format!("/{}", parts[1..].join("/"))
                } else {
                    "/".to_string()
                };
                Ok((snap, "/latest".to_string(), normalize_snap_path(&rel_path)))
            }
            ["snapshots", dir_name] | ["snapshots", dir_name, ..] => {
                let snap = self
                    .snapshot_by_dir_name(dir_name)
                    .ok_or_else(|| anyhow!("Снапшот не найден"))?;
                let rel_path = if parts.len() > 2 {
                    format!("/{}", parts[2..].join("/"))
                } else {
                    "/".to_string()
                };
                Ok((
                    snap.clone(),
                    format!("/snapshots/{}", snap.dir_name),
                    normalize_snap_path(&rel_path),
                ))
            }
            _ => Err(anyhow!("Путь не найден")),
        }
    }
}

pub fn status(state: &crate::scheduler::State) -> MountStatus {
    let mut guard = state.mount.lock().unwrap();
    if guard.as_ref().is_some_and(|session| session.handle.is_finished()) {
        *guard = None;
    }
    guard
        .as_ref()
        .map(|session| session.info.clone())
        .unwrap_or_default()
}

pub async fn stop(state: Arc<crate::scheduler::State>) -> anyhow::Result<MountStatus> {
    let session = { state.mount.lock().unwrap().take() };
    if let Some(mut session) = session {
        if let Some(pid) = session.info.pid.clone() {
            let name = session.info.profile_name.clone().unwrap_or_default();
            let unc = session.info.unc_path.clone().unwrap_or_default();
            restlex_core::log_profile(&pid, "INFO", &format!("mount webdav stop: {}", unc));
            state.push_log(format!(
                "[{}] mount stop: {}",
                Local::now().format("%H:%M:%S"),
                name
            ));
        }
        if let Some(tx) = session.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = session.handle.await;
    }
    Ok(MountStatus::default())
}

pub async fn start(
    state: Arc<crate::scheduler::State>,
    pid: String,
) -> anyhow::Result<MountStatus> {
    if let Some(current) = {
        let guard = state.mount.lock().unwrap();
        guard
            .as_ref()
            .filter(|session| !session.handle.is_finished())
            .map(|session| session.info.clone())
    } {
        if current.pid.as_deref() == Some(pid.as_str()) {
            return Ok(current);
        }
        stop(state.clone()).await?;
    }

    use restlex_core::i18n::tr;
    let en = state.cfg.globals().en();
    let profile = state
        .cfg
        .profile(&pid)
        .ok_or_else(|| anyhow!(tr(en, "Профиль не найден", "Profile not found")))?;
    let server_name = webdav_server_name();

    #[cfg(windows)]
    ensure_webclient_service();

    let fs = Arc::new(MountFs::new(&state.cfg, pid.clone(), &profile).await?);
    let port = find_free_port()?;
    // Только 127.0.0.1. Раньше был 0.0.0.0: смонтировали снапшот — и весь
    // репозиторий бэкапов доступен на чтение любому хосту в сети.
    // Windows WebClient по UNC `\\localhost@port\` ходит локально, mount работает.
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .with_context(|| if en {
            format!("Failed to open 127.0.0.1:{port}")
        } else {
            format!("Не удалось открыть 127.0.0.1:{port}")
        })?;

    let unc_path = format!(r"\\{}@{}\DavWWWRoot", server_name, port);
    let info = MountStatus {
        active: true,
        pid: Some(pid.clone()),
        profile_name: Some(profile.name.clone()),
        port: Some(port),
        unc_path: Some(unc_path),
        started_at: Some(Local::now().to_rfc3339()),
        // Кред нет: WebClient не отдаёт Basic по plain HTTP. GUI должен
        // подключаться `net use <UNC>` БЕЗ /user и /pass.
        note: Some(
            tr(
                en,
                "Read-only WebDAV mount (localhost, без авторизации)",
                "Read-only WebDAV mount (localhost, no authentication)",
            )
            .to_string(),
        ),
        username: None,
        password: None,
    };
    restlex_core::log_profile(
        &pid,
        "INFO",
        &format!(
            "mount webdav start: {}",
            info.unc_path.clone().unwrap_or_default()
        ),
    );
    state.push_log(format!(
        "[{}] mount start: {}",
        Local::now().format("%H:%M:%S"),
        profile.name
    ));

    let (tx, rx) = oneshot::channel::<()>();
    let app = Router::new()
        .route("/", any(handle_webdav))
        .route("/*path", any(handle_webdav))
        .with_state(fs);
    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
        {
            tracing::error!("mount webdav: {err}");
        }
    });

    state.mount.lock().unwrap().replace(MountSession {
        info: info.clone(),
        shutdown: Some(tx),
        handle,
    });

    Ok(info)
}

async fn handle_webdav(
    State(fs): State<Arc<MountFs>>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
) -> Response {
    let path = normalize_http_path(uri.path());
    let body_text = match to_bytes(body, 64 * 1024).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).trim().to_string(),
        Err(err) => format!("<body read error: {err}>"),
    };
    log_request(&fs, &method, &path, &headers, &body_text);
    match method.as_str() {
        "OPTIONS" => options_response(),
        "PROPFIND" => match handle_propfind(&fs, &path, &headers).await {
            Ok(resp) => resp,
            Err(err) => error_response(StatusCode::NOT_FOUND, &err.to_string()),
        },
        "GET" => match handle_get(&fs, &path, false).await {
            Ok(resp) => resp,
            Err(err) => error_response(StatusCode::NOT_FOUND, &err.to_string()),
        },
        "HEAD" => match handle_get(&fs, &path, true).await {
            Ok(resp) => resp,
            Err(err) => error_response(StatusCode::NOT_FOUND, &err.to_string()),
        },
        _ => readonly_response(),
    }
}

async fn handle_get(fs: &MountFs, path: &str, head_only: bool) -> anyhow::Result<Response> {
    let entry = fs.stat_path(path).await?;
    if entry.is_dir {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::OK;
        apply_common_headers(resp.headers_mut());
        resp.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("httpd/unix-directory"),
        );
        resp.headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
        resp.headers_mut()
            .insert(LAST_MODIFIED, header_date(entry.modified));
        resp.headers_mut()
            .insert(ETAG, HeaderValue::from_str(&etag_string(&entry))?);
        return Ok(resp);
    }

    let body = if head_only {
        Body::empty()
    } else {
        Body::from(fs.read_file(path).await?)
    };
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::OK;
    apply_common_headers(resp.headers_mut());
    resp.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    resp.headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from_str(&entry.size.to_string())?);
    resp.headers_mut()
        .insert(LAST_MODIFIED, header_date(entry.modified));
    resp.headers_mut()
        .insert(ETAG, HeaderValue::from_str(&etag_string(&entry))?);
    Ok(resp)
}

async fn handle_propfind(fs: &MountFs, path: &str, headers: &HeaderMap) -> anyhow::Result<Response> {
    let depth = headers
        .get("Depth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("0");
    let entry = fs.stat_path(path).await?;
    let mut entries = vec![entry.clone()];
    if entry.is_dir && depth != "0" {
        entries.extend(fs.list_dir(path).await?);
    }

    let href_base = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .map(|host| format!("http://{host}"));

    let mut xml = String::from(r#"<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:">"#);
    for item in entries {
        xml.push_str("<d:response>");
        let href_path = encode_href_path(&item.href_path, item.is_dir);
        let href = match &href_base {
            Some(base) => format!("{base}{href_path}"),
            None => href_path,
        };
        xml.push_str(&format!(
            "<d:href>{}</d:href>",
            xml_escape(&href)
        ));
        xml.push_str("<d:propstat><d:prop>");
        xml.push_str(&format!(
            "<d:displayname>{}</d:displayname>",
            xml_escape(&item.name)
        ));
        xml.push_str(&format!(
            "<d:getlastmodified>{}</d:getlastmodified>",
            header_date_string(item.modified)
        ));
        xml.push_str(&format!(
            "<d:creationdate>{}</d:creationdate>",
            creation_date_string(item.modified)
        ));
        xml.push_str(&format!(
            "<d:getetag>{}</d:getetag>",
            xml_escape(&etag_string(&item))
        ));
        if item.is_dir {
            xml.push_str("<d:resourcetype><d:collection/></d:resourcetype>");
            xml.push_str("<d:getcontentlength>0</d:getcontentlength>");
            xml.push_str("<d:getcontenttype>httpd/unix-directory</d:getcontenttype>");
        } else {
            xml.push_str("<d:resourcetype/>");
            xml.push_str(&format!(
                "<d:getcontentlength>{}</d:getcontentlength>",
                item.size
            ));
            xml.push_str("<d:getcontenttype>application/octet-stream</d:getcontenttype>");
        }
        xml.push_str(
            "<d:lockdiscovery/>\
             <d:supportedlock>\
               <d:lockentry><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype></d:lockentry>\
               <d:lockentry><d:lockscope><d:shared/></d:lockscope><d:locktype><d:write/></d:locktype></d:lockentry>\
             </d:supportedlock>",
        );
        xml.push_str("</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>");
        xml.push_str("</d:response>");
    }
    xml.push_str("</d:multistatus>");

    let mut resp = Response::new(Body::from(xml));
    *resp.status_mut() = StatusCode::from_u16(207)?;
    apply_common_headers(resp.headers_mut());
    resp.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    Ok(resp)
}

fn options_response() -> Response {
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = StatusCode::OK;
    apply_common_headers(resp.headers_mut());
    resp
}

fn readonly_response() -> Response {
    let mut resp = Response::new(Body::from("Mount доступен только на чтение"));
    *resp.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
    apply_common_headers(resp.headers_mut());
    resp
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let mut resp = Response::new(Body::from(message.to_string()));
    *resp.status_mut() = status;
    apply_common_headers(resp.headers_mut());
    resp.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp
}

fn apply_common_headers(headers: &mut HeaderMap) {
    headers.insert(HeaderNameExt::dav(), HeaderValue::from_static("1"));
    headers.insert(
        HeaderNameExt::ms_author_via(),
        HeaderValue::from_static("DAV"),
    );
    headers.insert(
        ALLOW,
        HeaderValue::from_static("OPTIONS, GET, HEAD, PROPFIND"),
    );
}

/// `en` — язык текста ошибки (уходит в ответ REST API).
fn parse_snapshots(raw: &str, en: bool) -> anyhow::Result<Vec<SnapshotInfo>> {
    use restlex_core::i18n::tr;
    let value: serde_json::Value = serde_json::from_str(raw).context(tr(
        en,
        "Не удалось разобрать список снапшотов",
        "Failed to parse the snapshot list",
    ))?;
    let snaps = value
        .as_array()
        .ok_or_else(|| anyhow!(tr(en, "Ожидался массив снапшотов", "Expected an array of snapshots")))?;

    let mut result = Vec::with_capacity(snaps.len());
    for snap in snaps {
        let id = snap
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!(tr(en, "У снапшота нет id", "Snapshot has no id")))?
            .to_string();
        let short_id = snap
            .get("short_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| id.chars().take(8).collect());
        let time = parse_dt(snap.get("time").and_then(|v| v.as_str()), Local::now());
        result.push(SnapshotInfo {
            id,
            short_id,
            dir_name: String::new(),
            time,
        });
    }
    Ok(result)
}

fn parse_dt(raw: Option<&str>, fallback: DateTime<Local>) -> DateTime<Local> {
    raw.and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|dt| dt.with_timezone(&Local))
        .unwrap_or(fallback)
}

fn normalize_snap_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.replace('\\', "/").split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            let _ = parts.pop();
            continue;
        }
        parts.push(part.to_string());
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

fn normalize_http_path(path: &str) -> String {
    normalize_snap_path(&percent_decode(path))
}

fn parent_path(path: &str) -> String {
    let path = normalize_snap_path(path);
    if path == "/" {
        return "/".to_string();
    }
    let mut parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let _ = parts.pop();
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

fn file_name(path: &str) -> String {
    path.trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn find_free_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let (Some(h1), Some(h2)) = (hex_val(bytes[idx + 1]), hex_val(bytes[idx + 2])) {
                out.push((h1 << 4) | h2);
                idx += 3;
                continue;
            }
        }
        if bytes[idx] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[idx]);
        }
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(ch: u8) -> Option<u8> {
    match ch {
        b'0'..=b'9' => Some(ch - b'0'),
        b'a'..=b'f' => Some(ch - b'a' + 10),
        b'A'..=b'F' => Some(ch - b'A' + 10),
        _ => None,
    }
}

fn encode_href_path(path: &str, is_dir: bool) -> String {
    let path = if path.is_empty() { "/" } else { path };
    let mut out = String::with_capacity(path.len() + 8);
    for byte in path.as_bytes() {
        match *byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'/' => out.push(*byte as char),
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    if is_dir && !out.ends_with('/') {
        out.push('/');
    }
    out
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn header_date(dt: DateTime<Local>) -> HeaderValue {
    HeaderValue::from_str(&header_date_string(dt))
        .unwrap_or_else(|_| HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"))
}

fn header_date_string(dt: DateTime<Local>) -> String {
    dt.with_timezone(&Utc)
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

fn creation_date_string(dt: DateTime<Local>) -> String {
    dt.with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn etag_string(entry: &VfsEntry) -> String {
    let kind = if entry.is_dir { "d" } else { "f" };
    format!("\"{}-{}-{}\"", kind, entry.size, entry.modified.timestamp())
}

fn log_request(fs: &MountFs, method: &Method, path: &str, headers: &HeaderMap, body_text: &str) {
    let auth = if headers.contains_key(AUTHORIZATION) {
        "yes"
    } else {
        "no"
    };
    let depth = headers
        .get("Depth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-");
    let ua = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-");
    let line = format!(
        "mount webdav request [{}]: {} {} auth={} depth={} host={} ua={}",
        fs.profile_name,
        method,
        path,
        auth,
        depth,
        headers
            .get(HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-"),
        ua
    );
    tracing::info!("{line}");
    log_mount_profile(fs, "INFO", &line);
    if method.as_str() == "PROPFIND" && !body_text.is_empty() {
        // Тело пишется в журнал профиля ДО любых проверок, а порт открыт любому
        // локальному процессу: без обрезки одним циклом PROPFIND с 64 КБ тела
        // журнал раздувается неограниченно. Для отладки хватает начала.
        const MAX_BODY_LOG: usize = 512;
        let shown: String = body_text.chars().take(MAX_BODY_LOG).collect();
        let body_line = format!(
            "mount webdav propfind body [{}]: {}{}",
            fs.profile_name,
            shown,
            if body_text.chars().count() > MAX_BODY_LOG { "…(обрезано)" } else { "" }
        );
        tracing::info!("{body_line}");
        log_mount_profile(fs, "INFO", &body_line);
    }
}

fn log_mount_profile(fs: &MountFs, level: &str, message: &str) {
    restlex_core::log_profile(&fs.pid, level, message);
}

/// Имя хоста в UNC-пути монтирования. Строго localhost: сервер слушает только
/// 127.0.0.1, а COMPUTERNAME резолвился бы в LAN-адрес и mount ломался бы.
fn webdav_server_name() -> String {
    "localhost".to_string()
}

#[cfg(windows)]
fn ensure_webclient_service() {
    let _ = std::process::Command::new("sc")
        .args(["start", "WebClient"])
        .output();
}

struct HeaderNameExt;

impl HeaderNameExt {
    fn dav() -> axum::http::header::HeaderName {
        axum::http::header::HeaderName::from_static("dav")
    }

    fn ms_author_via() -> axum::http::header::HeaderName {
        axum::http::header::HeaderName::from_static("ms-author-via")
    }
}

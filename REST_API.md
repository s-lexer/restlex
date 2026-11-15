# REST API v1 — Рекомендации по интеграции

## Обзор

Агент (`restlex-agent`) слушает HTTP на `127.0.0.1` (только loopback).  
**Bearer-токен требуется на ВСЕХ маршрутах** — и `/api/v1/*`, и легаси-маршрутах
GUI (`/status`, `/log`, `/run/:pid`, `/mount/*`, `/reload`, `/last-run`,
`/rest-server/*`, `/update/*`). Открытым остался единственный маршрут —
`GET /health` (проверка живости, отдаёт `ok`).

> Изменение поведения. Раньше легаси-маршруты были без аутентификации: любой
> локальный пользователь мог запустить бэкап, смонтировать репозиторий или
> подсунуть обновление службе, работающей под LocalSystem. Клиент, ходивший по
> ним без заголовка, теперь получает `401` — добавьте `Authorization`.

---

## Обнаружение порта

Агент пишет актуальный порт в `%ProgramData%\restlex\api.port` при каждом старте.  
используйте 17345 а если он не доступен то читайте api.port — агент может использовать fallback-порт при занятости основного.

```rust
fn agent_port() -> u16 {
    std::fs::read_to_string(r"C:\ProgramData\restlex\api.port")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(17345)
}
```

---

## Аутентификация

Токен лежит в `config.json` (поле `api_token`), файл — в
`%ProgramData%\RestLex\config.json`. Передавайте в каждом запросе:

```
Authorization: Bearer <api_token>
```

Сравнение на сервере constant-time — защита от timing-атак.

### Откуда берётся токен

**Агент генерирует его сам при старте службы**, если поле пустое: 32 случайных
байта из CSPRNG в hex (64 hex-символа) дописываются в `config.json`
read-modify-write под file-lock (чужие ключи не затираются). То есть отдельно
«выдавать» токен интегратору не нужно и заводить свой — тоже: **единый источник
правды — файл**, оттуда его читают и GUI, и CLI, и внешний клиент
(K-AutoAdmin). Оба процесса под SYSTEM на одной машине, так что чтение
`config.json` доступно.

Порядок для клиента:

1. Прочитать `api_token` из `%ProgramData%\RestLex\config.json`.
2. Если значение начинается с `enc:` — это DPAPI (machine scope, без entropy),
   расшифровать `CryptUnprotectData`. Шифрование включается глобальным флагом
   `encrypt_secrets`, поэтому в конфиге может лежать и плейнтекст — поддержите
   оба варианта.
3. Если поле пустое (агент ещё не стартовал или не смог записать файл) — агент
   принимает `update_api_key` как запасной ключ. Это именно fallback: ключ
   обновлений одинаков на всём парке, полагаться на него не нужно.
4. Оба пустые → API отвечает `403` на всё, кроме `/health`.

Токен меняется только вместе с `config.json` — перечитывайте файл, если получили
`401` (например, после переустановки агента).

Храните токен в своём конфиге/переменной окружения, не в коде.

---

## Как работают пароли и учётные данные

**Пароли не передаются через API.** Все учётные данные (пароль репозитория, ключи S3/B2/Azure) хранятся в профилях на машине где запущен агент. Вы только говорите агенту «запусти профиль X» — он сам знает куда и с каким паролем бэкапить.

Схема:
```
Ваш сервис  →  POST /api/v1/profiles/{pid}/backup  →  restlex-agent
                                                            ↓
                                                   restic + пароль из профиля
                                                            ↓
                                                       репозиторий
```

---

## Как запустить бэкап — пошагово

### Шаг 1 — Узнать pid профиля

```http
GET http://127.0.0.1:17345/api/v1/profiles
Authorization: Bearer ваш_токен
```

Ответ:
```json
{
  "ok": true,
  "profiles": [
    { "pid": "cf5b349e", "name": "Documents", "repo_type": "local", "last_status": "ok" },
    { "pid": "a1b2c3d4", "name": "Database",  "repo_type": "s3",    "last_status": "ok" }
  ]
}
```

`pid` — постоянный идентификатор профиля. Сохраните его в своём конфиге, чтобы не запрашивать каждый раз.

### Шаг 2 — Запустить бэкап

```http
POST http://127.0.0.1:17345/api/v1/profiles/cf5b349e/backup
Authorization: Bearer ваш_токен
```

Ответ `200 OK`:
```json
{ "ok": true, "pid": "cf5b349e" }
```

Бэкап запущен в фоне. Если бэкап уже идёт — вернётся `409 Conflict`:
```json
{ "ok": false, "error": "backup already running" }
```

### Шаг 3 — Проверить результат

```http
GET http://127.0.0.1:17345/api/v1/profiles/cf5b349e/status
Authorization: Bearer ваш_токен
```

Ответ пока бэкап идёт:
```json
{ "ok": true, "running": true, "last_run": null }
```

Ответ после завершения:
```json
{
  "ok": true,
  "running": false,
  "last_run": {
    "status": "ok",
    "timestamp": "2026-05-01T18:30:00+03:00",
    "files_new": 12,
    "snapshot_id": "a1b2c3d4"
  }
}
```

`last_run.status` — `"ok"`, `"failed"` или `"crashed"`.

### Полный пример на Rust

```rust
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct StatusResp {
    ok: bool,
    running: bool,
    last_run: Option<serde_json::Value>,
}

async fn run_backup_and_wait(pid: &str, token: &str) -> anyhow::Result<serde_json::Value> {
    let port = agent_port();
    let base = format!("http://127.0.0.1:{port}/api/v1");

    let client = Client::builder()
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("Authorization", format!("Bearer {token}").parse()?);
            h
        })
        .timeout(Duration::from_secs(10))
        .build()?;

    // Запустить бэкап
    let resp = client.post(format!("{base}/profiles/{pid}/backup")).send().await?;
    if resp.status() == 409 {
        anyhow::bail!("бэкап уже запущен");
    }

    // Ждать завершения (максимум 4 часа)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4 * 3600);
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;

        let s: StatusResp = client
            .get(format!("{base}/profiles/{pid}/status"))
            .send().await?
            .json().await?;

        if !s.running {
            return Ok(s.last_run.unwrap_or_default());
        }
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("таймаут ожидания бэкапа");
        }
    }
}

fn agent_port() -> u16 {
    std::fs::read_to_string(r"C:\ProgramData\restlex\api.port")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(17345)
}
```

---

## Как получить список снимков

Снимки — медленная операция (restic опрашивает репозиторий), поэтому она возвращает async job.

```rust
// Запросить список снимков
let job_resp: serde_json::Value = client
    .get(format!("{base}/snapshots/{pid}"))
    .send().await?.json().await?;
let job_id = job_resp["job_id"].as_str().unwrap();

// Поллить до готовности
loop {
    tokio::time::sleep(Duration::from_secs(2)).await;
    let job: serde_json::Value = client
        .get(format!("{base}/jobs/{job_id}"))
        .send().await?.json().await?;

    match job["job"]["status"].as_str().unwrap_or("") {
        "done"   => { println!("{}", job["job"]["result"]["snapshots"]); break; }
        "failed" => { eprintln!("ошибка: {}", job["job"]["result"]["error"]); break; }
        _        => continue,
    }
}
```

Ответ `result.snapshots` — массив объектов restic:
```json
[
  { "id": "a1b2c3d4", "time": "2026-05-01T18:30:00Z", "paths": ["C:\\Documents"], "tags": [] },
  { "id": "e5f6g7h8", "time": "2026-04-30T02:00:00Z", "paths": ["C:\\Documents"], "tags": [] }
]
```

---

## Маршруты

| Метод | Путь | Описание |
|-------|------|----------|
| `GET`    | `/api/v1/profiles`              | Список всех профилей |
| `POST`   | `/api/v1/profiles`              | **Создать профиль** (→ 201 + pid) |
| `GET`    | `/api/v1/profiles/{pid}`        | Детали профиля |
| `PUT`    | `/api/v1/profiles/{pid}`        | **Обновить профиль** |
| `DELETE` | `/api/v1/profiles/{pid}`        | **Удалить профиль** |
| `POST`   | `/api/v1/profiles/{pid}/backup` | Запустить бэкап (игнорирует расписание) |
| `POST`   | `/api/v1/profiles/{pid}/pause`  | **Снять с расписания** (sched.enabled=false) |
| `POST`   | `/api/v1/profiles/{pid}/resume` | **Вернуть на расписание** (sched.enabled=true) |
| `GET`    | `/api/v1/profiles/{pid}/status` | Статус (running + last-run) |
| `POST`   | `/api/v1/profiles/{pid}/stop`   | **Прервать идущий бэкап или restore** |
| `POST`   | `/api/v1/profiles/{pid}/test`   | **Проверить доступность репозитория** (→ async job) |
| `POST`   | `/api/v1/profiles/{pid}/restore` | **Восстановление из снимка** (→ async job) |
| `GET`    | `/api/v1/profiles/{pid}/snapshots` | Список снимков (алиас, → async job) |
| `GET`    | `/api/v1/snapshots/{pid}`       | Список снимков (→ async job) |
| `GET`    | `/api/v1/jobs/{job_id}`         | Статус async job |
| `GET`    | `/api/v1/settings`              | **Глобальные настройки** (без секретов) |
| `PATCH`  | `/api/v1/settings`              | **Частичный патч настроек** |
| `POST`   | `/api/v1/reload`                | **Перечитать конфиг с диска** |
| `GET`    | `/health`                       | Живость агента (**единственный маршрут без токена**) |

Легаси-маршруты GUI (`/status`, `/log`, `/run/:pid`, `/mount/status`,
`/mount/start/:pid`, `/mount/stop`, `/reload`, `/last-run[/:pid]`,
`/rest-server/{status,start,stop}`, `/update/{status,check,apply}`) остались на
месте, но теперь тоже за Bearer-токеном. Для интеграции предпочитайте `/api/v1`:
у легаси-маршрутов нет стабильного контракта ответа.

---

## Глобальные настройки — `GET` / `PATCH /api/v1/settings`

Добавлены в agent 1.8.7. Эквивалент CLI `restlex.exe set <key> <value>` без
`--profile`.

`GET` отдаёт настройки БЕЗ секретов: вместо `tg_token`, `report_api_key`,
`update_api_key`, `api_token` возвращается `<key>_set: true|false` — так же,
как `has_password` у профиля.

```http
GET /api/v1/settings
Authorization: Bearer <api_token>
```
```json
{"ok": true, "settings": {
  "restic": "C:\\ProgramData\\RestLex\\restic.exe",
  "global_no_scan": false,
  "global_vss_timeout_s": 300,
  "tg_token_set": true,
  "api_token_set": true
}}
```

`PATCH` накладывает ТОЛЬКО присланные ключи; остальное на диске не трогается
(read-modify-write под file-lock, параллельная правка из GUI не теряется).

```http
PATCH /api/v1/settings
Content-Type: application/json

{"global_vss_timeout_s": 600, "tg_chat": "-1001234567890"}
```

Ответ — то же тело, что и у `GET`, уже с новыми значениями. Ключи с суффиксом
`_set` отклоняются с 400: это вывод `GET`, а не поле конфига.

**Патчить можно только ключи из белого списка** (иначе `403` со списком в теле
ответа):

`tg_chat`, `tg_group`, `tg_fatal`, `tg_file`, `tg_prune`, `global_no_scan`,
`global_vss_timeout_s`, `global_pre_backup_cmd`, `global_backup_timeout_hours`,
`global_post_backup_check`, `auto_update`, `update_check_interval_m`,
`net_retry_count`, `net_retry_interval_s`, `temp`, `cache_cleanup`.

Список именно белый, а не чёрный. Агент работает под LocalSystem, поэтому
запись произвольного поля по сети равносильна выполнению произвольного кода:
`restic` — это путь к запускаемому бинарнику, а `default_password_file` и
`encrypt_secrets` управляют обращением с секретами. Такие поля меняются только
локально — в GUI или через `restlex.exe set` на самой машине.

Неверный тип значения (например, строка вместо числа) — `400`, а не `500`.

---

## Прервать операцию — `POST /api/v1/profiles/{pid}/stop`

Прерывает **и бэкап, и restore** — что из них сейчас идёт по профилю. Убивает
именно тот `restic.exe`, который принадлежит этому профилю (по PID,
запомненному при старте), а не все процессы restic на машине. Внутри PID
бэкапа и PID restore лежат под разными ключами, `stop` смотрит оба; перепутать
операции нельзя — бэкап и restore делят один флаг `running`, второй запрос по
занятому профилю получает `409`.

Результат прерванного прогона помечается как отменённый, а не как сбой —
Telegram-алерт о сбое не отправляется:

- бэкап → статус прогона `cancelled` (в логе профиля `CANCELLED`);
- restore → job переходит в `status: "cancelled"` (не `failed`, см. ниже).

- `200 {"ok": true, "killed": <pid>}` — прервали
- `200 {"ok": true, "killed": 0}` — процесс успел завершиться сам между
  проверкой и kill (профиль считался занятым, убивать уже нечего)
- `409 {"ok": false, "error": "backup is not running"}` — по профилю ничего не
  идёт (текст исторический, относится и к restore)
- `404` — профиля нет

---

## Проверить репозиторий — `POST /api/v1/profiles/{pid}/test`

Async job: выполняет `restic cat config` — разом проверяет сеть, учётные
данные бэкенда и пароль репозитория. Дешевле, чем `snapshots`.

```json
{"ok": true, "job_id": "..."}
```

Результат забирать через `GET /api/v1/jobs/{job_id}`. Недоступный репозиторий
— это НЕ ошибка job'а: job завершится `done`, а в `result` будет
`{"reachable": false, "error": "..."}`. Успех — `{"reachable": true}`.

---

## Восстановление — `POST /api/v1/profiles/{pid}/restore`

Async job: запускает `restic restore` в указанный каталог. Что восстанавливать,
берётся из списка снимков (`GET /api/v1/profiles/{pid}/snapshots`, он же
`GET /api/v1/snapshots/{pid}`).

Тело запроса:

```json
{
  "snapshot_id": "latest",
  "target": "D:\\restore\\2026-09-04",
  "include": ["/C/Documents/proj"]
}
```

| Поле | Обяз. | Описание |
|------|-------|----------|
| `snapshot_id` | нет (по умолчанию `latest`) | `latest` либо id снимка из списка снимков |
| `target`      | **да** | Абсолютный каталог назначения: `X:\...` или UNC `\\server\share\...` |
| `include`     | нет | Маски путей внутри снимка (`--include`). Синоним поля — `paths`. Пустой список = восстановить весь снимок |

Ответ: `202 {"ok": true, "job_id": "..."}`. Дальше — поллинг
`GET /api/v1/jobs/{job_id}`, как у остальных async job'ов.

Пока job в статусе `running`, в `result` лежит прогресс:

```json
{"percent": 0.42, "files_restored": 120, "total_files": 300,
 "bytes_restored": 10485760, "total_bytes": 25000000}
```

По завершении:

- `done` → `{"summary": {"files_restored": …, "bytes_restored": …, "seconds_elapsed": …, "errors": []}, "target": "D:\\restore\\2026-09-04"}`
- `failed` → `{"error": "..."}` (учётные данные репозитория в тексте замаскированы)
- `cancelled` → restore прервали через `/stop`; в цели остались частично
  восстановленные файлы (см. «Отмена» ниже)

### Валидация (ошибки 4xx)

Параметры уезжают в командную строку restic, поэтому проверяются строго. Тело
ошибки — всегда JSON-объект с `ok: false` и `error`:

- `snapshot_id` — только `latest` или 4–64 символа `[A-Za-z0-9]`; иначе
  `400 {"ok": false, "error": "bad snapshot_id"}`
- `target` — непустой абсолютный путь (`X:\…` или `\\server\share\…`), без
  компонентов `..`, без ведущего `-` и без NUL; иначе `400 {"ok": false,
  "error": "<причина>"}` (`target is empty`, `target must be an absolute path`,
  `target must not contain '..'` и т.п.)
- `include` — не более 64 элементов, каждый без ведущего `-` и без NUL; пустые
  строки отбрасываются

Прочие коды: `400 {"ok": false, "error": "bad pid"}` — неверный формат pid;
`404 {"ok": false, "error": "profile not found"}`;
`409 {"ok": false, "error": "profile is busy"}` — по профилю уже идёт бэкап или
другой restore.

**Тело, которое не разобрал axum, до нашей валидации не доходит** и отвечает
не нашим JSON, а простым текстом:

- JSON синтаксически битый → `400`;
- JSON валидный, но поле не того типа (например `"include": "C:\\x"` вместо
  массива) → **`422 Unprocessable Entity`**, а не `400`;
- нет заголовка `Content-Type: application/json` → `415`.

Клиенту: не считайте `4xx` без поля `ok` нашей ошибкой валидации — это признак
неверно сформированного запроса.

### Отмена

Отдельного эндпоинта нет: идущий restore прерывает тот же
**`POST /api/v1/profiles/{pid}/stop`**, что и бэкап — `200 {"ok": true,
"killed": <pid>}`.

PID процесса restore живёт под собственным ключом `restore:<pid>`, отдельно от
бэкапа (иначе завершившийся restore снимал бы регистрацию идущего бэкапа), но
`stop` проверяет оба ключа. Убить не ту операцию нельзя: бэкап и restore делят
флаг `running` профиля, поэтому одновременно живёт только одна из них.

Прерванный job переходит в **`status: "cancelled"`** (не `failed` и не `done`):

```json
{"cancelled": true, "partial": true, "target": "D:\\restore\\2026-09-04",
 "error": "restore failed with exit code 1"}
```

`partial: true` — напоминание, что в целевом каталоге лежат **частично**
восстановленные файлы; их надо удалить или восстановить заново. Поле `error`
содержит текст от убитого restic (или `null`) и на статус не влияет.

### Время жизни job'а

Restore крупного репозитория идёт часами. Job в статусе `running` сборщиком
мусора **не удаляется вообще** — TTL (1 час) отсчитывается только от момента
завершения. То есть результат гарантированно доступен ещё час после того, как
restore закончился, независимо от его длительности.

---

## Развёртывание на чистой машине (CLI, не API)

Пока служба не установлена, API отвечать некому — bootstrap делается запуском
команд на хосте (у Recondeck для этого есть общий канал команд):

```cmd
restlex.exe install --all
restlex.exe import-config \\share\etalon.json
restlex.exe profiles
restlex.exe set repo_password "<pw>" --profile <pid>
```

Эталон снимается на образцовой машине через
`restlex.exe export-config <path> --no-secrets` — без паролей и токенов, такой
файл можно класть на общий ресурс. `import-config` и `set` сами дёргают
`/reload`, перезапускать службу не нужно. Полный список ключей —
`restlex.exe --help`.

---

## Управление профилями (write-эндпоинты)

Добавлены в agent 1.8.2+. Позволяют внешнему клиенту (K-AutoAdmin) создавать и
редактировать профили бэкапа, а не только запускать их. Все требуют тот же
Bearer-токен, что и остальные `/api/v1/*`.

### Создать профиль — `POST /api/v1/profiles`

Тело — поля профиля (все опциональны кроме `name` и `repo_path`):

```json
{
  "name": "Документы 1С",
  "sources": ["D:\\1C\\base", "C:\\Users\\Public\\Documents"],
  "exclude": ["*.tmp", "*.log"],
  "repo_type": "sftp",
  "repo_path": "sftp:backup@nas.local:/pool/restic/rd11",
  "repo_password": "секрет-репозитория",
  "use_vss": true,
  "retention": { "keep-daily": 7, "keep-weekly": 4, "keep-monthly": 6 },
  "sched": { "freq": "daily", "time": "03:00", "forget": true, "enabled": true },
  "cred_s3_key": "…", "cred_s3_secret": "…"
}
```

- `repo_type` — один из `local|sftp|s3|b2|azure|gs|rest` (иной → `400`).
- Облачные ключи (`cred_s3_key`, `cred_b2_*`, `cred_az_*`, `cred_gs_*`, `cred_sftp_*`,
  `cred_rest_*`) кладутся в тело плоскими полями.
- `last_run`/`last_status`/`created`/`updated` — server-owned, из тела игнорируются.

Ответ `201`:
```json
{ "ok": true, "pid": "3fa8c210", "profile": { … безопасное представление … } }
```

### Обновить — `PUT /api/v1/profiles/{pid}`

Тело — те же поля профиля. `created` и история прогонов сохраняются агентом
(клиент их не перезаписывает). `404` если профиля нет.

**Merge-семантика секретов (важно).** `GET` маскирует `repo_password` и `cred_*`
(отдаёт `has_password`/`creds_set`, не значения), поэтому при флоу
GET→правка→PUT в теле секретов нет. Правило:

- `repo_password`: **пустой/отсутствующий → сохраняется прежний**; непустой → заменяет.
- каждый `cred_*` (и любой `extra`-ключ): **пустой/отсутствующий → сохраняется прежний**;
  непустой → заменяет.

Так одно изменённое поле не затирает пароль репозитория и облачные ключи. Чтобы
СМЕНИТЬ секрет — пришлите его непустым; «очистить» секрет через PUT нельзя
(шлите пустое = «оставить»). Профиль с пустым `repo_password` И без глобального
password-file отклоняется (`400`) — чтобы не сохранить заведомо битый профиль.

### Удалить — `DELETE /api/v1/profiles/{pid}`

`{ "ok": true, "pid": "…" }`. `404` если профиля нет.

### Пауза / возобновление — `POST …/pause` · `POST …/resume`

Выставляют `sched.enabled` в `false`/`true` **без потери** частоты/времени.
Профиль на паузе не запускается планировщиком, но `POST …/backup` работает.
Старые `profiles.json` без поля `enabled` читаются как «не на паузе».

```json
{ "ok": true, "pid": "…", "enabled": false }
```

### Что видно в GET (не секретно)

`GET /api/v1/profiles[/{pid}]` теперь отдаёт также `repo_path`, `use_vss`,
`retention`, `sched`, `enabled` — для редактирования из UI. `repo_password` и
значения `cred_*` **по-прежнему не отдаются**; вместо них — `has_password: bool`
и `creds_set: [имена заполненных ключей]`.

---

## Async Jobs

Долгие операции (`snapshots`, `test`, `restore`) возвращают `202 Accepted` с `job_id`.  
Поллите `/api/v1/jobs/{job_id}` пока `status != "running"`:

```rust
loop {
    let job = client.get(url).send().await?.json::<Job>().await?;
    match job.status.as_str() {
        "running"   => tokio::time::sleep(Duration::from_secs(2)).await,
        "done"      => break Ok(job.result),
        // "cancelled" — restore прервали через /stop: не успех и не сбой
        _           => break Err(job.result),
    }
}
```

Job в статусе `running` не удаляется вообще — сколько бы ни шёл. После
завершения (`done`/`failed`/`cancelled`) он живёт **час**, затем убирается GC-циклом агента
(проход раз в 5 минут). Часа хватает клиенту с редким поллингом; 10 минут, как
было раньше, не хватало — результат исчезал до того, как его успевали забрать.

---

## HTTP-коды

| Код | Ситуация |
|-----|----------|
| `200` | Успех |
| `202` | Job принят в работу |
| `400` | Ошибка валидации тела/параметров (наш JSON с `ok: false`) либо синтаксически битый JSON (текст от axum) |
| `401` | Токена нет или он неверный — на любом маршруте, кроме `GET /health` |
| `403` | `api_token` и `update_api_key` не заданы (API отключено) |
| `404` | Профиль / job не найден |
| `409` | По профилю уже идёт операция (бэкап или restore) |
| `415` | Нет заголовка `Content-Type: application/json` (от axum) |
| `422` | Тело — валидный JSON, но не той формы (от axum, без поля `ok`) |

---

## Что никогда не возвращается

Ответы не содержат:
- `repo_password`
- S3/B2/Azure ключи (`cred_s3_key`, `cred_s3_secret` и т.п.)
- `tg_token`

Поле `extra` профиля полностью исключено из `/api/v1/*` ответов.

---

## Рекомендации для Rust-клиента

**Зависимости** — минимальный набор:

```toml
reqwest = { version = "0.12", features = ["json"] }
tokio   = { version = "1", features = ["full"] }
serde   = { version = "1", features = ["derive"] }
serde_json = "1"
```

**Клиент** — создавайте один `reqwest::Client` с вшитым заголовком авторизации:

```rust
let client = reqwest::Client::builder()
    .default_headers({
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("Authorization", format!("Bearer {token}").parse()?);
        h
    })
    .timeout(Duration::from_secs(10))
    .build()?;
```

**Транспорт** — API слушает только `127.0.0.1`, TLS не нужен.  
Если ваш сервис на другой машине — пробрасывайте порт через SSH-туннель или VPN, не открывайте агент наружу напрямую.

**Таймауты** — для `/backup` и `/snapshots` достаточно 10 с (они мгновенно возвращают job_id).  
Для поллинга job'а — 5–10 с на итерацию, суммарный таймаут зависит от размера репо.

**Ошибки** — всегда проверяйте `ok: false` в теле, даже при `200`:

```rust
#[derive(Deserialize)]
struct ApiResp { ok: bool, error: Option<String> }
```

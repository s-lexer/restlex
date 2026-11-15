#!/usr/bin/env python3
"""
Restlex v1.0 — GUI-клиент для restic (Python/PyQt5)

Основные возможности:
- Светлая тема
- Несколько профилей с полной изоляцией процессов
- Бэкап с VSS, --no-scan, pre/post CMD-командами
- Maintenance: repair index, rebuild-index, check
- Wake-on-LAN перед бэкапом
- Планировщик задач Windows (запуск от текущего пользователя или SYSTEM)
- Telegram-уведомления (per-profile): фатальные ошибки, ошибки файлов, очистка, таймаут
- WebDAV / встроенный Explorer для просмотра снапшотов
- Drag & drop восстановление с обработкой конфликтов
- Авто-загрузка restic при первом запуске
- Семантическое версионирование (начиная с 1.0.0)
"""
import sys, os, json, shutil, uuid, threading, re, tempfile, subprocess, hashlib, time, zipfile, ctypes, socket, queue
import urllib.request, urllib.error, urllib.parse
from pathlib import Path, PurePosixPath
from datetime import datetime
from typing import Optional, Dict, List
from collections import deque
import i18n

try:
    import requests as _req; _HAS_REQUESTS = True
except ImportError:
    _HAS_REQUESTS = False

from PyQt5.QtWidgets import (
    QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QTabWidget, QPushButton, QInputDialog, QFileDialog,
    QListWidget, QListWidgetItem, QLineEdit, QSpinBox, QCheckBox,
    QLabel, QGroupBox, QSplitter, QTreeWidget, QTreeWidgetItem,
    QAbstractItemView, QHeaderView, QTextEdit, QPlainTextEdit, QMessageBox,
    QScrollArea, QStackedWidget, QToolButton, QDialog, QFrame,
    QProgressBar, QMenu, QAction, QSizePolicy, QComboBox, QFileIconProvider,
    QRadioButton, QButtonGroup, QTimeEdit, QProgressDialog, QToolBar, QFormLayout, QGridLayout,
    QStyledItemDelegate, QStyleOptionViewItem, QStyle
)
from PyQt5.QtCore import Qt, QProcess, QObject, pyqtSignal, QTimer, QProcessEnvironment, QMimeData, QPoint, QPointF, QRectF, QSize, QTime, QFileInfo, QEventLoop
from PyQt5.QtGui import QFont, QIcon, QColor, QDrag, QDragEnterEvent, QDropEvent, QCursor, QPalette, QPainter, QPixmap, QPen, QPainterPath
try:
    import pythoncom
    import win32con
    import win32gui
    import win32process
    import win32com.client
    _HAS_SHELL_EMBED = sys.platform == "win32"
except Exception:
    pythoncom = None
    win32con = None
    win32gui = None
    win32process = None
    win32com = None
    _HAS_SHELL_EMBED = False

# ── Light palette ────────────────────────────────────────────────────────────
STYLE = """
QMainWindow, QWidget { background:#f5f5f5; color:#1a1a1a; }
QTabWidget::pane { border:1px solid #cccccc; background:#ffffff; }
QTabBar::tab { background:#e0e0e0; color:#333; padding:5px 7px; min-width:0; font-size:8.5pt;
               border:1px solid #ccc; border-bottom:none; margin-right:2px; }
QTabBar::tab:selected { background:#ffffff; color:#000; font-weight:bold; }
QGroupBox { border:1px solid #cccccc; border-radius:4px; margin-top:8px;
            background:#ffffff; font-weight:bold; }
QGroupBox::title { subcontrol-origin:margin; left:10px; padding:0 4px; }
QPushButton { background:#e8e8e8; border:1px solid #bbb; border-radius:3px;
              padding:4px 10px; color:#1a1a1a; }
QPushButton:hover { background:#d0d8ff; border-color:#7090cc; }
QPushButton:pressed { background:#b0c0f0; }
QPushButton:disabled { background:#eeeeee; color:#aaaaaa; border-color:#dddddd; }
QLineEdit { background:#ffffff; border:1px solid #cccccc; border-radius:3px;
            padding:3px 5px; color:#1a1a1a; }
QLineEdit:focus { border-color:#5080cc; }
QLineEdit[readOnly="true"] { background:#f0f0f0; }
QListWidget, QTreeWidget { background:#ffffff; border:1px solid #cccccc;
                            alternate-background-color:#f8f8f8; color:#1a1a1a; }
QListWidget::item:selected, QTreeWidget::item:selected {
    background:#4a80d0; color:white; }
QHeaderView::section { background:#eeeeee; border:1px solid #cccccc;
                        padding:4px; font-weight:bold; color:#333; }
QScrollBar:vertical { width:12px; background:#f0f0f0; }
QScrollBar::handle:vertical { background:#cccccc; border-radius:6px; min-height:20px; }
QProgressBar { border:1px solid #cccccc; border-radius:3px; background:#f0f0f0;
               text-align:center; height:6px; }
QProgressBar::chunk { background:#4CAF50; border-radius:2px; }
QComboBox { background:#ffffff; border:1px solid #cccccc; border-radius:3px;
            padding:3px 6px; color:#1a1a1a; }
QComboBox::drop-down { border-left:1px solid #cccccc; width:20px; background:#e8e8e8; }
QComboBox::down-arrow { width:0; height:0; border-left:5px solid transparent; border-right:5px solid transparent; border-top:6px solid #444; margin:auto; }
QComboBox QAbstractItemView { background:#ffffff; border:1px solid #aaaaaa;
                               selection-background-color:#4a80d0; }
QLabel { color:#1a1a1a; background:transparent; }
QCheckBox, QRadioButton { background:transparent; }
QToolButton { background:#e8e8e8; border:1px solid #bbb; border-radius:3px; padding:2px; }
QToolButton:hover { background:#d0d8ff; }
QMenu { background:#ffffff; border:1px solid #aaaaaa; }
QMenu::item { padding:4px 20px; }
QMenu::item:selected { background:#4a80d0; color:white; }
QSplitter::handle { background:#dddddd; }
"""

LOG_LINE_RE = re.compile(
    r"^\[(?P<ts>[^\]]+)\]\s*\[(?P<level>[^\]]+)\](?:\s*\[(?P<channel>[^\]]+)\])?\s*(?P<message>.*)$"
)

MAX_LOG_BLOCKS = 800
MAX_BACKUP_LOG_BLOCKS = 600
MAX_BACKUP_REPORT_LINES = 400
PROGRAMDATA_ROOT = Path(os.environ.get("ProgramData", "C:\\ProgramData"))
APP_DATA_DIRNAME = "RestLex"
LEGACY_APP_DATA_DIRNAME = "restlex"
try:
    from version import APP_VERSION
except Exception:
    APP_VERSION = "1.0.0"
APP_TITLE = "RestLex"
WINDOWS_APP_ID = "RestLex.GUI"
WOL_PING_INTERVAL_DEF = 2      # сек между пингами при ожидании хоста после WOL
WOL_WAIT_TIMEOUT_DEF = 60      # сек общего ожидания (было 30 попыток × 2 сек)
SHUTDOWN_DELAY_DEF = 60        # сек таймера shutdown /s /t N после успешного бэкапа
SHUTDOWN_CHECK_MARGIN = 15     # запас поверх таймера: живы после него → выключение не состоялось


def normalize_log_channel(channel: str) -> str:
    value = str(channel or "").strip().lower()
    if value == "scheduler":
        return "info"
    return value if value in ("info", "debug", "error") else "debug"


def format_log_line(ts: str, level: str, message: str, channel: str = "debug", show_channel: bool = True) -> str:
    channel = normalize_log_channel(channel)
    if show_channel and channel.upper() != str(level or "").upper():
        return f"[{ts}][{level}][{channel}] {message}"
    return f"[{ts}][{level}] {message}"


def parse_log_line(line: str) -> Optional[dict]:
    raw = str(line or "").rstrip("\r\n")
    if not raw:
        return None
    m = LOG_LINE_RE.match(raw)
    if not m:
        return {
            "ts": "",
            "level": "INFO",
            "channel": "",
            "message": raw,
            "raw": raw,
        }
    data = m.groupdict()
    return {
        "ts": data.get("ts") or "",
        "level": data.get("level") or "INFO",
        "channel": normalize_log_channel(data.get("channel") or "debug") if data.get("channel") else "",
        "message": data.get("message") or "",
        "raw": raw,
    }


def infer_legacy_log_channel(message: str, current: str = "debug") -> str:
    low = str(message or "").strip().lower()
    if not low:
        return normalize_log_channel(current)
    # Ошибки — всегда показывать
    if low.startswith("❌ ") or "access is denied" in low or "permission denied" in low:
        return "error"
    if "unable to create lock" in low or "already locked" in low or "lock was created" in low:
        return "error"
    if low.startswith("error:") or low.startswith("fatal:"):
        return "error"
    if "forget exit" in low and "exit 0" not in low:
        return "error"
    # Info — важные события
    if "=== бэкап" in low or "=== очистка" in low:
        return "info"
    if "scheduled backup" in low or "=== forget" in low or "backup done" in low:
        return "info"
    if "бэкап заверш" in low or "очистка заверш" in low:
        return "info"
    if "правила хранения" in low or "инициализация не удалась" in low:
        return "info"
    # Restic сводка — показывать в info
    if low.startswith("files:") or low.startswith("dirs:"):
        return "info"
    if "added to the repo" in low or "snapshot" in low and "saved" in low:
        return "info"
    if low.startswith("processed ") and "files" in low:
        return "info"
    if "unlock:" in low or "removed" in low and "snapshot" in low:
        return "info"
    # Debug — технические детали
    if "=== восстановление" in low:
        return "debug"
    if "mount " in low or "webdav" in low or "agent " in low or "telegram" in low:
        return "debug"
    if "web-отч" in low or "restlexagent" in low:
        return "debug"
    return normalize_log_channel(current)


def app_icon() -> QIcon:
    candidates = []
    if getattr(sys, "frozen", False):
        candidates.append(Path(getattr(sys, "_MEIPASS", Path(sys.executable).resolve().parent)) / "icon.ico")
    candidates.append(Path(__file__).resolve().with_name("icon.ico"))
    for path in candidates:
        try:
            if path and path.exists():
                icon = QIcon(str(path))
                if not icon.isNull():
                    return icon
        except Exception:
            continue
    if sys.platform == "win32" and getattr(sys, "frozen", False):
        try:
            icon = QFileIconProvider().icon(QFileInfo(str(Path(sys.executable))))
            if not icon.isNull():
                return icon
        except Exception:
            pass
    return QIcon()


def line_icon(kind: str, color: str = "#444444", size: int = 20) -> QIcon:
    """Векторные значки (корзина / обновить), рисуются без шрифтов-эмодзи."""
    scale = 2
    pm = QPixmap(size * scale, size * scale)
    pm.setDevicePixelRatio(scale)
    pm.fill(Qt.transparent)
    p = QPainter(pm)
    p.setRenderHint(QPainter.Antialiasing, True)
    p.setPen(QPen(QColor(color), 1.6, Qt.SolidLine, Qt.RoundCap, Qt.RoundJoin))
    p.setBrush(Qt.NoBrush)
    k = size / 20.0
    p.scale(k, k)
    if kind == "trash":
        p.drawLine(QPointF(4, 6), QPointF(16, 6))
        p.drawLine(QPointF(8, 6), QPointF(8, 4))
        p.drawLine(QPointF(12, 6), QPointF(12, 4))
        p.drawLine(QPointF(8, 4), QPointF(12, 4))
        body = QPainterPath()
        body.moveTo(5.5, 6); body.lineTo(6.5, 16.5); body.lineTo(13.5, 16.5); body.lineTo(14.5, 6)
        p.drawPath(body)
        p.drawLine(QPointF(8.5, 8.5), QPointF(8.8, 14))
        p.drawLine(QPointF(11.5, 8.5), QPointF(11.2, 14))
    elif kind == "refresh":
        p.drawArc(QRectF(4.5, 4.5, 11, 11), 40 * 16, 280 * 16)
        p.setBrush(QColor(color))
        p.drawPolygon(QPointF(15.8, 3.2), QPointF(16.6, 8.6), QPointF(11.4, 7.4))
    p.end()
    return QIcon(pm)


def set_windows_app_id():
    if sys.platform != "win32":
        return
    try:
        ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID(WINDOWS_APP_ID)
    except Exception:
        pass

# ═══════════════════════════════════════════════════════════════════════════
# DPAPI helpers — encrypt/decrypt secrets in config files
# ═══════════════════════════════════════════════════════════════════════════
import base64 as _b64

_GLOBAL_SECRET_KEYS  = {"tg_token", "report_api_key", "update_api_key", "api_token"}
_PROFILE_SECRET_KEYS = {"repo_password"}

# Всё, что нельзя выпускать в plaintext-JSON при export-config --no-secrets.
# Шире, чем _*_SECRET_KEYS: те задают DPAPI-шифрование на диске, а здесь —
# креды бэкендов, которые на диске лежат открыто, но в шаренный файл попасть не должны.
# rest_user СОЗНАТЕЛЬНО не в списке: это логин, а не секрет. Вычищая его,
# дефолтный экспорт ломал перенос конфигурации «как было» — на целевой машине
# приходилось вручную вспоминать имя пользователя rest-репозитория.
_EXPORT_SECRET_PROFILE_KEYS = _PROFILE_SECRET_KEYS | {
    "sftp_p", "s3_s", "b2_k", "az_k", "rest_pass",
}
_EXPORT_SECRET_GLOBAL_KEYS = _GLOBAL_SECRET_KEYS
_EXPORT_SECRET_WEB_KEYS = {
    "admin_password", "viewer_password", "api_key", "secret_key", "tg_token",
}
# Поля webserver-config.json, которые шифруются DPAPI на диске (см.
# WebServerManager.load_config/save_config). Rust-крейт webserver читает их
# через restlex_core::crypto::decrypt_secret — enc: и плейнтекст оба валидны.
_WEBSERVER_SECRET_KEYS = (
    "admin_password", "viewer_password", "api_key", "tg_token", "secret_key",
)

def _strip_url_password(url: str) -> str:
    """rest:http://user:pass@host/repo -> rest:http://host/repo

    Режем userinfo целиком, до ПОСЛЕДНЕГО '@' перед путём: пароль может
    содержать сырой '@' (URL не всегда собран через _upd_rest_url), и срез
    по первому разделителю оставил бы хвост пароля в файле.
    """
    if not isinstance(url, str) or "@" not in url:
        return url
    return re.sub(r"(//)[^/\s]*@", r"\1", url, count=1)


def _strip_export_secrets(data: dict) -> dict:
    """Обнуляет секреты в структуре export-config (не мутирует исходник)."""
    out = dict(data)
    out["global"] = {
        k: ("" if k in _EXPORT_SECRET_GLOBAL_KEYS else v)
        for k, v in (data.get("global") or {}).items()
    }
    profiles = {}
    for pid, prof in (data.get("profiles") or {}).items():
        if not isinstance(prof, dict):
            profiles[pid] = prof
            continue
        p = {
            k: ("" if k in _EXPORT_SECRET_PROFILE_KEYS else v)
            for k, v in prof.items()
        }
        # у rest-репозитория пароль зашит прямо в repo_path
        if p.get("repo_type") == "rest":
            p["repo_path"] = _strip_url_password(p.get("repo_path", ""))
        profiles[pid] = p
    out["profiles"] = profiles
    out["webserver"] = {
        k: ("" if k in _EXPORT_SECRET_WEB_KEYS else v)
        for k, v in (data.get("webserver") or {}).items()
    }
    return out


def _atomic_write_json(fp, obj):
    """Атомарная запись JSON: уникальный per-pid tmp + fsync + os.replace.
    Уникальный tmp исключает коллизию GUI↔агента; os.replace атомарен и
    не оставляет окна без файла (в отличие от unlink+move). С ретраями."""
    fp = Path(fp)
    payload = json.dumps(obj, indent=2, ensure_ascii=False)
    t = fp.with_name(f"{fp.name}.{os.getpid()}.tmp")
    last_err = None
    for attempt in range(5):
        try:
            with open(t, "w", encoding="utf-8") as f:
                f.write(payload)
                f.flush()
                os.fsync(f.fileno())
            os.replace(str(t), str(fp))
            return
        except Exception as e:
            last_err = e
            time.sleep(0.1)
    try:
        if t.exists(): t.unlink()
    except Exception: pass
    # Не глотаем молча: в windowed-exe print уходит в никуда, а пользователь
    # видит «сохранено», хотя на диске старые данные. Журнал + предупреждение.
    try:
        with open(fp.parent / "save-errors.log", "a", encoding="utf-8") as lf:
            lf.write(f"{datetime.now().isoformat()} {fp.name}: {last_err}\n")
    except Exception:
        pass
    print(f"save err: {last_err}")
    app = QApplication.instance()
    if app is not None and threading.current_thread() is threading.main_thread():
        QMessageBox.warning(None, "RestLex",
            f"Не удалось сохранить {fp.name}:\n{last_err}\n\n"
            f"Изменения могут быть потеряны.")

class _ConfigLock:
    """Межпроцессный эксклюзивный лок на .config.lock, совместимый с Rust-агентом
    (обе стороны используют LockFileEx на одном файле — fs2 в Rust). Нужен потому,
    что GUI (сессия пользователя) и служба-агент (LocalSystem) правят одни и те же
    config.json/profiles.json. Best-effort: при любом сбое работаем без лока, а не
    блокируем правки. Ждём лок ограниченно (~5с), чтобы не морозить UI-поток."""
    _PATH = PROGRAMDATA_ROOT / APP_DATA_DIRNAME / ".config.lock"
    _EXCL = 0x00000002          # LOCKFILE_EXCLUSIVE_LOCK
    _NOWAIT = 0x00000001        # LOCKFILE_FAIL_IMMEDIATELY

    def __enter__(self):
        self._h = None
        try:
            import win32file, win32con, pywintypes
            try:
                self._PATH.parent.mkdir(parents=True, exist_ok=True)
            except Exception:
                pass
            self._h = win32file.CreateFile(
                str(self._PATH),
                win32con.GENERIC_READ | win32con.GENERIC_WRITE,
                win32con.FILE_SHARE_READ | win32con.FILE_SHARE_WRITE,
                None, win32con.OPEN_ALWAYS, 0, None)
            for _ in range(50):  # ~5с максимум, потом работаем без лока
                try:
                    win32file.LockFileEx(self._h, self._EXCL | self._NOWAIT,
                                         0, 0xFFFFFFFF, 0xFFFFFFFF, pywintypes.OVERLAPPED())
                    return self
                except pywintypes.error:
                    time.sleep(0.1)
            self._close()  # не смогли взять — не морозим UI, пишем без лока
        except Exception:
            self._close()
        return self

    def _close(self):
        if self._h is not None:
            try:
                import win32file
                win32file.CloseHandle(self._h)
            except Exception:
                pass
            self._h = None

    def __exit__(self, *exc):
        if self._h is not None:
            try:
                import win32file, pywintypes
                win32file.UnlockFileEx(self._h, 0, 0xFFFFFFFF, 0xFFFFFFFF, pywintypes.OVERLAPPED())
            except Exception:
                pass
            self._close()
        return False

class _DpapiError(Exception):
    """DPAPI-шифрование недоступно/сломалось. Раньше _dpapi_encrypt в этом случае
    молча возвращал ПЛЕЙНТЕКСТ, а UI рапортовал «секреты зашифрованы» — пароли
    лежали открыто. Теперь сбой пробрасывается наверх и запись отменяется."""


class _ConfigReadError(Exception):
    """Файл конфига существует, но прочитать/разобрать его не удалось (залочен
    агентом, битый JSON, нет прав). Писать поверх НЕЛЬЗЯ — затрём чужие ключи."""


# Хук уведомления пользователя из слоя Config (устанавливается GUI).
_CONFIG_WARN_HOOK = None


def _config_warn(msg: str):
    cb = _CONFIG_WARN_HOOK
    if cb is None:
        try:            # headless/CLI — хотя бы в stderr, а не молча
            print(f"[restlex] {msg}", file=sys.stderr)
        except Exception:
            pass
        return
    try:
        cb(msg)
    except Exception:
        pass


# Живой Config процесса — источник Bearer-токена для REST API агента.
_AGENT_TOKEN_CFG = None

# Текущее состояние тумблера «Шифрование секретов» — нужно слоям, у которых нет
# ссылки на Config (RestServerManager). Обновляется в Config.__init__/sg.
_ENCRYPT_SECRETS = False


def _dpapi_local_machine_flag() -> int:
    """CRYPTPROTECT_LOCAL_MACHINE = 4 (WinCrypt.h). В части сборок pywin32
    константы нет в самом win32crypt — она живёт в win32cryptcon, и обращение
    к win32crypt.CRYPTPROTECT_LOCAL_MACHINE роняло AttributeError → _DpapiError:
    шифрование «работало» только на бумаге, а секреты писались плейнтекстом при
    включённой галке. Берём константу устойчиво, с числовым фолбэком."""
    try:
        import win32crypt
        v = getattr(win32crypt, "CRYPTPROTECT_LOCAL_MACHINE", None)
        if v is not None:
            return int(v)
    except Exception:
        pass
    try:
        import win32cryptcon
        v = getattr(win32cryptcon, "CRYPTPROTECT_LOCAL_MACHINE", None)
        if v is not None:
            return int(v)
    except Exception:
        pass
    return 4


def _dpapi_encrypt(plain: str) -> str:
    if not plain or plain.startswith("enc:"):
        return plain
    try:
        import win32crypt
        enc = win32crypt.CryptProtectData(
            plain.encode("utf-8"), "", None, None, None,
            _dpapi_local_machine_flag()  # any process on this machine can decrypt
        )
        return "enc:" + _b64.b64encode(enc).decode()
    except Exception as e:
        raise _DpapiError(str(e))


def _dpapi_probe() -> None:
    """Проверка, что DPAPI реально работает: шифруем и расшифровываем маркер.
    Бросает _DpapiError, если шифрование недоступно."""
    marker = "restlex-dpapi-probe"
    enc = _dpapi_encrypt(marker)
    if not enc.startswith("enc:") or _dpapi_decrypt(enc) != marker:
        raise _DpapiError("проверочная расшифровка не совпала")

def _dpapi_decrypt(s: str) -> str:
    if not isinstance(s, str) or not s.startswith("enc:"):
        return s  # plain text — will be encrypted on next save (migration)
    try:
        import win32crypt
        data = _b64.b64decode(s[4:])
        _, dec = win32crypt.CryptUnprotectData(
            data, None, None, None,
            _dpapi_local_machine_flag()
        )
        return dec.decode("utf-8")
    except Exception:
        return s  # decryption failed — return as-is

# ═══════════════════════════════════════════════════════════════════════════
# Config
# ═══════════════════════════════════════════════════════════════════════════
class Config:
    APP   = APP_DATA_DIRNAME
    BASE  = PROGRAMDATA_ROOT / APP
    LEGACY_BASE = PROGRAMDATA_ROOT / LEGACY_APP_DATA_DIRNAME
    CFG   = BASE / "config.json"
    PROF  = BASE / "profiles.json"
    LOGS  = BASE / "logs"

    def __init__(self):
        self._migrate_legacy_base()
        for p in [self.BASE, self.LOGS]: p.mkdir(parents=True, exist_ok=True)
        self._corrupted = []
        self._read_failed = []
        self._write_failures = 0   # счётчик неудачных sg() — см. _save_global
        self._g  = self._ld(self.CFG,  {})
        self._pr = self._ld(self.PROF, {})
        global _ENCRYPT_SECRETS, _AGENT_TOKEN_CFG
        _ENCRYPT_SECRETS = bool(self._g.get("encrypt_secrets", False))
        _AGENT_TOKEN_CFG = self   # источник Bearer-токена для agent_api_request
        self._ready = True        # после старта о карантине сообщаем сразу

    @property
    def write_failures(self) -> int:
        """Сколько раз sg() отказался записать настройку (лок/битый файл/DPAPI).
        Вызывающий код сравнивает значение до и после серии присваиваний, чтобы
        не рапортовать «✅ сохранено», когда на диск ничего не легло."""
        return self._write_failures

    def encrypt_secrets_enabled(self) -> bool:
        # Дефолт OFF (галка снята), пока пользователь явно не включит шифрование.
        return bool(self._g.get("encrypt_secrets", False))

    def _migrate_legacy_base(self):
        legacy = self.LEGACY_BASE
        current = self.BASE
        if legacy == current or not legacy.exists():
            return
        try:
            if not current.exists():
                shutil.move(str(legacy), str(current))
                return
        except Exception:
            pass
        try:
            if os.path.normcase(str(legacy)) == os.path.normcase(str(current)):
                temp = legacy.parent / f"{self.APP}_rename_{uuid.uuid4().hex[:8]}"
                shutil.move(str(legacy), str(temp))
                shutil.move(str(temp), str(current))
        except Exception:
            pass

    def _quarantine(self, fp, reason: str) -> None:
        """Битый (не залоченный) файл конфига отложить в *.bad-<время> и дать
        пересоздать его с нуля. Иначе strict-чтение навсегда переводило GUI в
        режим «только чтение»: каждый sg()/save_profile ловил _ConfigReadError и
        молча выходил, а «перезапустите GUI» не помогало. Логика согласована с
        Rust-агентом (patch_globals)."""
        msg = f"{fp.name}: повреждён ({reason})"
        try:
            bad = fp.with_name(f"{fp.name}.bad-{datetime.now().strftime('%Y%m%d-%H%M%S')}")
            shutil.move(str(fp), str(bad))
            msg = (f"{fp.name}: повреждён ({reason}) — отложен как {bad.name}; "
                   f"работаем со значениями по умолчанию, файл будет создан "
                   f"заново при первом сохранении настроек")
        except Exception as e:
            msg = f"{fp.name}: повреждён ({reason}), и отложить его не удалось: {e}"
        if msg not in self._corrupted:
            self._corrupted.append(msg)
        if getattr(self, "_ready", False):
            _config_warn(msg)

    def _ld(self, fp, d, strict=False):
        """strict=True — «файла нет» по-прежнему легально (вернём default), а вот
        не смогли ПРОЧИТАТЬ существующий файл (лок агента, нет прав) →
        _ConfigReadError: писать поверх нельзя, затрём чужие ключи.
        Раньше глоталось ЛЮБОЕ исключение: config.json на миг залочен агентом →
        default {} → следующая запись сохраняла файл из одного ключа и стирала
        tg_token/report_url/api-ключи.
        Невалидный JSON — ДРУГОЙ случай: файл читается, но бесполезен, ждать
        нечего. Такой файл уводим в карантин и работаем дальше."""
        if not fp.exists(): return d
        try:
            raw = fp.read_text(encoding="utf-8")
        except Exception as e:
            entry = f"{fp.name}: {e}"
            if entry not in self._read_failed:   # sg() зовётся десятками — не растим список
                self._read_failed.append(entry)
            if strict:
                raise _ConfigReadError(entry)
            return d
        if not raw.strip():
            # Пустой файл — НЕ порча: так выглядит оборванная запись (антивирус,
            # сторонний инструмент, падение питания между truncate и write).
            # Карантинить рабочий конфиг из-за 0 байт нельзя — данные ещё могут
            # доехать. Обрабатываем как read-failure: strict-вызовы откажутся
            # писать поверх, остальные получат default.
            entry = f"{fp.name}: файл пуст (0 байт) — вероятно, запись оборвана"
            if entry not in self._read_failed:
                self._read_failed.append(entry)
            if strict:
                raise _ConfigReadError(entry)
            return d
        try:
            data = json.loads(raw)
            if not isinstance(data, dict):
                raise ValueError("не JSON-объект")
        except Exception as e:
            self._quarantine(fp, str(e))
            return d
        is_profiles = (fp == self.PROF)
        if is_profiles:
            for prof in data.values():
                if isinstance(prof, dict):
                    for k in _PROFILE_SECRET_KEYS:
                        if k in prof:
                            prof[k] = _dpapi_decrypt(prof[k])
        else:
            for k in _GLOBAL_SECRET_KEYS:
                if k in data:
                    data[k] = _dpapi_decrypt(data[k])
        return data

    def _sv(self, fp, data):
        import copy
        save = copy.deepcopy(data)
        # Шифруем секреты ТОЛЬКО если включён тумблер encrypt_secrets. При OFF
        # пишем плейнтекстом. (_ld расшифровывает enc: всегда — это безопасно и для
        # совместимости со старыми конфигами.) Решение синхронизировано с Rust-агентом.
        do_enc = bool(self._g.get("encrypt_secrets", False))
        is_profiles = (fp == self.PROF)
        if do_enc and is_profiles:
            for prof in save.values():
                if isinstance(prof, dict):
                    for k in _PROFILE_SECRET_KEYS:
                        if k in prof and isinstance(prof[k], str):
                            prof[k] = _dpapi_encrypt(prof[k])
        elif do_enc:
            for k in _GLOBAL_SECRET_KEYS:
                if k in save and isinstance(save[k], str):
                    save[k] = _dpapi_encrypt(save[k])
        _atomic_write_json(fp, save)

    def _sv_guarded(self, fp, data) -> bool:
        """_sv + честная обработка сбоя DPAPI: лучше НЕ записать файл и сказать
        об этом, чем записать секреты плейнтекстом при включённом шифровании."""
        try:
            self._sv(fp, data)
            return True
        except _DpapiError as e:
            _config_warn(
                f"DPAPI-шифрование недоступно ({e}).\n"
                f"Файл {fp.name} НЕ сохранён, чтобы секреты не попали на диск "
                f"открытым текстом. Отключите «Шифрование секретов» или "
                f"устраните проблему с DPAPI."
            )
            return False

    def sg(self, k, v) -> bool:
        # Read-modify-write под межпроцессным локом, как в save_profile: пишем
        # ТОЛЬКО свой ключ поверх свежего диска. Раньше сохранялся весь снимок,
        # снятый при старте процесса, и параллельная правка (другая копия GUI,
        # CLI `set` из скрипта развёртывания) молча терялась.
        # Возвращает True, только если значение реально легло на диск.
        with _ConfigLock():
            try:
                on_disk = self._ld(self.CFG, dict(self._g), strict=True)
            except _ConfigReadError as e:
                self._write_failures += 1
                _config_warn(
                    f"Не удалось прочитать config.json ({e}).\n"
                    f"Настройка «{k}» НЕ сохранена — запись отменена, чтобы не "
                    f"затереть остальные параметры. Повторите попытку."
                )
                return False
            on_disk[k] = v
            self._g = on_disk
            global _ENCRYPT_SECRETS
            _ENCRYPT_SECRETS = bool(self._g.get("encrypt_secrets", False))
            ok = self._sv_guarded(self.CFG, self._g)
            if not ok:
                self._write_failures += 1
            return ok
    def gg(self, k, d=""): return self._g.get(k, d)

    def managed_restic_path(self) -> Path:
        return self.BASE / "restic.exe"

    @property
    def restic(self):
        value = str(self.gg("restic", "") or "").strip()
        if value:
            return value
        managed = self.managed_restic_path()
        return str(managed) if managed.exists() else "restic"
    @restic.setter
    def restic(self,v):      self.sg("restic",v)
    @property
    def temp(self):          return self.gg("temp","")
    @temp.setter
    def temp(self,v):        self.sg("temp",v)
    @property
    def tg_token(self):      return self.gg("tg_token","")
    @tg_token.setter
    def tg_token(self,v):    self.sg("tg_token",v)
    @property
    def tg_chat(self):       return self.gg("tg_chat","")
    @tg_chat.setter
    def tg_chat(self,v):     self.sg("tg_chat",v)
    @property
    def tg_fatal(self):      return self.gg("tg_fatal",False)
    @tg_fatal.setter
    def tg_fatal(self,v):    self.sg("tg_fatal",v)
    @property
    def tg_file(self):       return self.gg("tg_file",False)
    @tg_file.setter
    def tg_file(self,v):     self.sg("tg_file",v)
    @property
    def tg_prune(self):      return self.gg("tg_prune",False)
    @tg_prune.setter
    def tg_prune(self,v):    self.sg("tg_prune",v)
    @property
    def tg_group(self):      return self.gg("tg_group","")
    @tg_group.setter
    def tg_group(self,v):    self.sg("tg_group",v)
    @property
    def report_url(self):    return self.gg("report_url","")
    @report_url.setter
    def report_url(self,v):  self.sg("report_url",v)
    @property
    def report_api_key(self):    return self.gg("report_api_key","")
    @report_api_key.setter
    def report_api_key(self,v):  self.sg("report_api_key",v)
    @property
    def agent_auto_update_url(self):     return self.gg("agent_auto_update_url","")
    @agent_auto_update_url.setter
    def agent_auto_update_url(self,v):   self.sg("agent_auto_update_url",v)
    @property
    def update_server_url(self):  return self.gg("update_server_url","http://1221.by:3266")
    @update_server_url.setter
    def update_server_url(self,v): self.sg("update_server_url",v)
    # Ключа в исходниках нет: общий сервер обновлений (public) отдаёт программы
    # без ключа, целостность — Authenticode у агента. Приватный сервер — ключ в настройках.
    _UPD_API_KEY_DEFAULT = ""
    @property
    def update_api_key(self):     return self.gg("update_api_key", self._UPD_API_KEY_DEFAULT)
    @update_api_key.setter
    def update_api_key(self,v):   self.sg("update_api_key", v or self._UPD_API_KEY_DEFAULT)
    @property
    def auto_update(self):        return bool(self.gg("auto_update", True))
    @auto_update.setter
    def auto_update(self,v):      self.sg("auto_update", bool(v))
    @property
    def api_token(self):
        """Bearer-токен для REST API v1 (/api/v1/*). Если пусто — используется update_api_key."""
        v = self.gg("api_token", "")
        return v if v else self.update_api_key
    @api_token.setter
    def api_token(self,v):        self.sg("api_token", v.strip())
    @property
    def global_no_scan(self):      return self.gg("global_no_scan",False)
    @global_no_scan.setter
    def global_no_scan(self,v):    self.sg("global_no_scan",v)
    @property
    def global_post_backup_check(self):      return self.gg("global_post_backup_check",False)
    @global_post_backup_check.setter
    def global_post_backup_check(self,v):    self.sg("global_post_backup_check",v)

    @property
    def global_vss_timeout_s(self):
        try:    return max(30, int(self.gg("global_vss_timeout_s", 300) or 300))
        except Exception: return 300
    @global_vss_timeout_s.setter
    def global_vss_timeout_s(self,v):        self.sg("global_vss_timeout_s", int(v))

    @property
    def global_check_host_only(self):        return self.gg("global_check_host_only",False)
    @global_check_host_only.setter
    def global_check_host_only(self,v):      self.sg("global_check_host_only",v)
    @property
    def global_pre_backup_cmd(self):      return self.gg("global_pre_backup_cmd","")
    @global_pre_backup_cmd.setter
    def global_pre_backup_cmd(self,v):    self.sg("global_pre_backup_cmd",v)
    @property
    def global_backup_timeout_hours(self):      return int(self.gg("global_backup_timeout_hours",0) or 0)
    @global_backup_timeout_hours.setter
    def global_backup_timeout_hours(self,v):    self.sg("global_backup_timeout_hours",int(v or 0))
    @property
    def cache_cleanup(self):      return bool(self.gg("cache_cleanup", True))
    @cache_cleanup.setter
    def cache_cleanup(self,v):    self.sg("cache_cleanup", bool(v))

    @property
    def restore_ui_mode(self):
        default = "old"
        value = str(self.gg("restore_ui_mode", default) or default).strip().lower()
        return value if value in ("old", "new") else default
    @restore_ui_mode.setter
    def restore_ui_mode(self,v):
        value = str(v or "").strip().lower()
        self.sg("restore_ui_mode", value if value in ("old", "new") else "old")

    def profiles(self):      return self._pr
    def profile(self, pid):  return self._pr.get(pid)

    def save_profile(self, pid, data):
        # Read-modify-write под межпроцессным локом: перечитываем свежий диск,
        # меняем ТОЛЬКО свой профиль и сохраняем агентовы last_run/last_status
        # (они могли обновиться на диске уже после старта GUI). Так мы не затираем
        # чужие профили и не откатываем статусы, которые пишет служба.
        with _ConfigLock():
            try:
                on_disk = self._ld(self.PROF, dict(self._pr), strict=True)
            except _ConfigReadError as e:
                _config_warn(
                    f"Не удалось прочитать profiles.json ({e}).\n"
                    f"Профиль НЕ сохранён — запись отменена, чтобы не затереть "
                    f"остальные профили. Повторите попытку."
                )
                return
            old = on_disk.get(pid) or self._pr.get(pid, {})
            data["created"] = old.get("created", datetime.now().isoformat())
            data["updated"] = datetime.now().isoformat()
            for k in ("last_run", "last_status"):   # эти поля — владение агента
                if old.get(k) is not None:
                    data[k] = old[k]
            on_disk[pid] = data
            self._pr = on_disk
            self._sv_guarded(self.PROF, self._pr)

    def del_profile(self, pid) -> bool:
        """True — профиль реально удалён с диска. Вызывающий код обязан сносить
        задачи планировщика ТОЛЬКО после True: иначе расписание удалено, а
        профиль остался, и бэкапы тихо перестают идти по графику."""
        with _ConfigLock():
            try:
                on_disk = self._ld(self.PROF, dict(self._pr), strict=True)
            except _ConfigReadError as e:
                _config_warn(
                    f"Не удалось прочитать profiles.json ({e}).\n"
                    f"Профиль НЕ удалён — запись отменена. Повторите попытку."
                )
                return False
            on_disk.pop(pid, None)
            self._pr = on_disk
            if not self._sv_guarded(self.PROF, self._pr):
                return False
        lf = self.LOGS / f"{pid}.log"
        try:
            if lf.exists(): lf.unlink()
        except Exception:
            pass  # лог держит агент/антивирус — не рвём удаление профиля
        return True

    def reencrypt_all(self, new_flag: Optional[bool] = None) -> bool:
        """Перешифровать все секреты на диске под флаг encrypt_secrets
        (вызывается при переключении тумблера). _sv сам решит шифровать или нет.
        Перед записью ПЕРЕЧИТЫВАЕМ оба файла: self._g/_pr — снимки, снятые при
        старте процесса, и запись ими поверх откатывала правки агента/Recondeck
        (last_run, изменённые профили) на всё время работы GUI.

        new_flag выставляется ЗДЕСЬ, а не отдельным sg() до вызова: флаг живёт в
        config.json, и если записать его первым, а profiles.json не записать, то
        флаг ON при плейнтекстовых профилях — и каждый последующий save_profile
        валится в _sv_guarded. Поэтому пишем profiles.json ПЕРВЫМ, config.json с
        флагом — вторым, и при любом сбое откатываем флаг в памяти."""
        global _ENCRYPT_SECRETS
        with _ConfigLock():
            try:
                g  = self._ld(self.CFG,  dict(self._g),  strict=True)
                pr = self._ld(self.PROF, dict(self._pr), strict=True)
            except _ConfigReadError as e:
                _config_warn(
                    f"Не удалось прочитать конфиг ({e}).\n"
                    f"Перешифровка секретов отменена — файлы не изменены."
                )
                return False
            prev = bool(self._g.get("encrypt_secrets", False))
            if new_flag is not None:
                g["encrypt_secrets"] = bool(new_flag)
            self._g, self._pr = g, pr
            _ENCRYPT_SECRETS = bool(self._g.get("encrypt_secrets", False))

            def _rollback() -> bool:
                global _ENCRYPT_SECRETS
                self._g["encrypt_secrets"] = prev
                _ENCRYPT_SECRETS = prev
                return False

            # Порядок записи зависит от НАПРАВЛЕНИЯ: первым идёт файл, чей
            # промежуточный результат безвреден (расшифровка enc: работает всегда,
            # а вот «флаг ON при плейнтекстовых профилях» ломает save_profile).
            #   ON  (False→True): profiles (уже enc:) → config (флаг True)
            #   OFF (True→False): config (флаг False) → profiles (ещё enc:)
            # В обоих случаях при сбое второй записи на диске остаётся валидное
            # сочетание, и _rollback правит только память.
            order = ((self.PROF, self._pr), (self.CFG, self._g))
            if not _ENCRYPT_SECRETS:
                order = ((self.CFG, self._g), (self.PROF, self._pr))
            for fp, data in order:
                if not self._sv_guarded(fp, data):
                    return _rollback()
        return True

    def log(self, pid, msg, lv="INFO", channel="debug"):
        ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        with open(self.LOGS/f"{pid}.log","a",encoding="utf-8") as f:
            f.write(format_log_line(ts, lv, msg, channel=channel, show_channel=True) + "\n")

    def _tail_lines(self, fp: Path, max_lines: int = 500, max_bytes: int = 1024 * 512) -> List[str]:
        if not fp.exists():
            return []
        with open(fp, "rb") as f:
            f.seek(0, os.SEEK_END)
            size = f.tell()
            read_size = min(size, max_bytes)
            f.seek(-read_size, os.SEEK_END if size else os.SEEK_SET)
            chunk = f.read(read_size).decode("utf-8", errors="replace")
        return list(deque(chunk.splitlines(True), maxlen=max_lines))

    def get_logs(self, pid, mode="debug"):
        lf = self.LOGS/f"{pid}.log"
        # Merge all sources: per-profile log + all _sched.log files for this profile
        sources: List[Path] = []
        if lf.exists():
            sources.append(lf)
        try:
            for p in self.LOGS.glob(f"{pid}_*_sched.log"):
                sources.append(p)
        except Exception:
            pass
        if not sources:
            return []
        # Read all + sort by timestamp
        raw_lines: List[str] = []
        for src in sources:
            try:
                raw_lines.extend(self._tail_lines(src, max_lines=800))
            except Exception:
                pass
        # Sort by [ts] prefix if present
        def _ts_key(line: str) -> str:
            m = LOG_LINE_RE.match(line)
            return (m.group("ts") if m else "").strip()
        raw_lines.sort(key=_ts_key)
        # Drop spammy restic JSON status lines from sched logs
        raw_lines = [l for l in raw_lines if '"message_type":"status"' not in l and '"message_type":"verbose_status"' not in l]
        mode = normalize_log_channel(mode)
        out = []
        legacy_channel = "debug"
        for raw in raw_lines[-1200:]:
            rec = parse_log_line(raw)
            if not rec:
                continue
            level_up = str(rec.get("level") or "").upper()
            level_channel = "error" if level_up in ("FAILED", "ERROR") else ("info" if level_up in ("SUCCESS",) else "")
            channel = rec.get("channel") or level_channel or infer_legacy_log_channel(rec.get("message",""), legacy_channel)
            legacy_channel = channel
            if mode == "info" and channel not in ("info", "error"):
                continue
            if mode == "error" and channel != "error":
                continue
            out.append(
                format_log_line(
                    rec.get("ts",""),
                    rec.get("level","INFO"),
                    rec.get("message",""),
                    channel=channel,
                    show_channel=(mode in ("debug", "error")),
                ) + "\n"
            )
        return out[-500:]

    def clr_logs(self, pid):
        lf = self.LOGS/f"{pid}.log"
        try:
            if lf.exists(): lf.unlink()
        except Exception:
            pass  # файл держит агент/антивирус — не рвём цепочку бэкапа


def _decode_restic(data: bytes) -> str:
    """Decode restic process output, falling back to Windows OEM/ANSI codepages."""
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        for enc in ("cp866", "cp1251"):
            try:
                return data.decode(enc)
            except Exception:
                continue
        return data.decode("utf-8", errors="replace")

# ═══════════════════════════════════════════════════════════════════════════
# RepoMutex — cross-process Windows mutex keyed by sha1(repo_path).
# Layer 0 защиты от параллельных бэкапов в один репо (smart preflight).
# ═══════════════════════════════════════════════════════════════════════════
class RepoMutex(QObject):
    acquired = pyqtSignal()
    waiting  = pyqtSignal()
    failed   = pyqtSignal(str)

    def __init__(self, repo_path: str, parent=None):
        super().__init__(parent)
        h = hashlib.sha1(repo_path.encode("utf-8", errors="replace")).hexdigest()[:16]
        self._name_global = f"Global\\RestLex_{h}"
        self._release_event = threading.Event()
        self._thread = None
        self._held = False

    def acquire_async(self):
        if self._thread and self._thread.is_alive():
            return
        self._release_event = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def release(self):
        self._release_event.set()

    def is_held(self) -> bool:
        return self._held

    def _run(self):
        try:
            k = ctypes.windll.kernel32
            k.CreateMutexW.restype  = ctypes.c_void_p
            k.CreateMutexW.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_wchar_p]
            k.WaitForSingleObject.restype  = ctypes.c_uint
            k.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint]
            k.ReleaseMutex.argtypes = [ctypes.c_void_p]
            k.CloseHandle.argtypes  = [ctypes.c_void_p]
            h = k.CreateMutexW(None, 0, self._name_global)
            if not h:
                # Local\ имеет отдельный namespace для каждой пользовательской
                # сессии и не защищает репозиторий от службы в другой сессии.
                self.failed.emit(f"CreateMutex Global err={k.GetLastError()}")
                return
            r = k.WaitForSingleObject(h, 0)
            if r == 0x102:  # WAIT_TIMEOUT — занят, ждём
                self.waiting.emit()
                r = k.WaitForSingleObject(h, 0xFFFFFFFF)
            if r not in (0, 0x80):  # WAIT_OBJECT_0 / WAIT_ABANDONED
                k.CloseHandle(h)
                self.failed.emit(f"WaitForSingleObject rc=0x{r:x}")
                return
            self._held = True
            self.acquired.emit()
            self._release_event.wait()
            try: k.ReleaseMutex(h)
            except Exception: pass
            k.CloseHandle(h)
            self._held = False
        except Exception as e:
            self._held = False
            self.failed.emit(str(e))


# ═══════════════════════════════════════════════════════════════════════════
# ResticWorker — password via RESTIC_PASSWORD env ONLY
# ═══════════════════════════════════════════════════════════════════════════
class ResticWorker(QObject):
    out  = pyqtSignal(str)
    err  = pyqtSignal(str)
    done = pyqtSignal(int, str)

    def __init__(self, restic, repo, password, temp="", extra=None):
        super().__init__()
        self.restic=restic; self.repo=repo; self.password=password
        self.temp=temp; self.extra=extra or {}
        self._stdout_pending = ""
        self._stderr_pending = ""
        self.proc = QProcess(self)
        self.proc.readyReadStandardOutput.connect(self._so)
        self.proc.readyReadStandardError.connect(self._se)
        self.proc.finished.connect(self._fin)
        self.proc.errorOccurred.connect(self._on_error_occurred)

    def _env(self):
        e = QProcessEnvironment.systemEnvironment()
        e.insert("RESTIC_REPOSITORY", self.repo)
        if not e.contains("LOCALAPPDATA"):
            e.insert("LOCALAPPDATA", os.environ.get("LOCALAPPDATA",
                str(Path.home()/"AppData"/"Local")))
        if self.password:
            e.insert("RESTIC_PASSWORD", self.password)
        if self.temp:
            for k in ("TMPDIR","TEMP","TMP"): e.insert(k, self.temp)
        for k,v in self.extra.items():
            if v: e.insert(k,v)
        return e

    # Репозиторий передаём ТОЛЬКО через env (RESTIC_REPOSITORY в _env):
    # rest-URL содержит user:pass@ и в argv виден любому в списке процессов.
    def _base(self): return []

    def _run(self, args):
        self._stdout_pending=""
        self._stderr_pending=""
        self.proc.setProcessEnvironment(self._env())
        self.proc.start(self.restic, args)

    def backup(self, srcs, vss=False, excl=None, no_scan=False, vss_timeout_s=0):
        a = self._base()+["backup", "--json"]
        if vss:
            a.append("--use-fs-snapshot")
            # Дефолт restic — 120с; на нагруженном сервере снимок тома не всегда
            # успевает, и бэкап падает целиком. Авто-сброс VSS остаётся запасным.
            if vss_timeout_s:
                a += ["-o", f"vss.timeout={int(vss_timeout_s)}s"]
        if no_scan: a.append("--no-scan")
        for p in (excl or []): a+=["--exclude",p]
        a+=srcs; self._run(a)

    def restore(self, sid, target, files=None):
        t=target.rstrip("\\")
        if len(t)==2 and t.endswith(":"): t+="\\"
        a=self._base()+["restore",sid,"--target",t]
        for f in (files or []): a+=["--include",f]
        self._run(a)

    def snapshots(self): self._run(self._base()+["snapshots","--json"])
    def ls(self,sid, path=None):
        a=self._base()+["ls","--json",sid]
        if path:
            a.append(path)
        self._run(a)
    def cat_cfg(self):   self._run(self._base()+["cat","config"])
    def init(self):      self._run(self._base()+["init"])
    def unlock(self):    self._run(self._base()+["unlock"])
    def forget_s(self,sid): self._run(self._base()+["forget","--prune",sid])

    def forget_pol(self, pol):
        a=self._base()+["forget"]
        for k,f in [("keep-last","--keep-last"),("keep-daily","--keep-daily"),
                    ("keep-weekly","--keep-weekly"),("keep-monthly","--keep-monthly"),
                    ("keep-yearly","--keep-yearly")]:
            if pol.get(k): a+=[f,str(pol[k])]
        a.append("--prune"); self._run(a)

    def repair_index(self): self._run(self._base()+["repair","index"])
    def rebuild_index(self): self._run(self._base()+["repair","index"])
    def check_repo(self, host_only=False):
        a = self._base() + ["check"]
        # --host требует restic >= 0.19.0; на старых упадёт с "unknown flag".
        # Имя берём то же, что restic пишет в снимок. При переименовании машины
        # старые снимки под этот фильтр не попадут — поэтому имя видно в логе.
        if host_only:
            host = socket.gethostname()
            self.out.emit(f"check ограничен снимками хоста: {host}")
            a += ["--host", host]
        self._run(a)

    def _so(self):
        d = _decode_restic(self.proc.readAllStandardOutput().data())
        self._stdout_pending = self._emit_lines(d, self._stdout_pending, self.out)

    def _se(self):
        d = _decode_restic(self.proc.readAllStandardError().data())
        self._stderr_pending = self._emit_lines(d, self._stderr_pending, self.err)

    def _emit_lines(self, data: str, pending: str, signal) -> str:
        if not data:
            return pending
        text = (pending + data).replace("\r\n", "\n").replace("\r", "\n")
        parts = text.split("\n")
        tail = parts.pop() if text and not text.endswith("\n") else ""
        for ln in parts:
            if ln.strip():
                signal.emit(ln.rstrip())
        return tail

    def _flush_pending(self, pending: str, signal) -> str:
        if pending.strip():
            signal.emit(pending.rstrip())
        return ""

    def _fin(self,code,_):
        self._so(); self._se()
        self._stdout_pending = self._flush_pending(self._stdout_pending, self.out)
        self._stderr_pending = self._flush_pending(self._stderr_pending, self.err)
        self.done.emit(code,"")

    def _on_error_occurred(self, error):
        # QProcess.FailedToStart (бинарник не найден/нет прав) не даёт finished —
        # без этого обработчика GUI виснет до внешнего watchdog-таймаута,
        # выдавая ложный "таймаут" вместо реальной причины.
        if error == QProcess.FailedToStart:
            self.err.emit(f"Не удалось запустить restic: файл не найден или недоступен ({self.restic})")
            self.done.emit(-1, "")

    def stop(self, wait_ms: int = 250):
        try:
            if self.proc.state() == QProcess.NotRunning:
                return True
            self.proc.blockSignals(True)
            self.proc.terminate()
            if not self.proc.waitForFinished(wait_ms):
                self.proc.kill()
                self.proc.waitForFinished(wait_ms)
        except Exception:
            pass
        if self.proc.state() != QProcess.NotRunning:
            return False
        try:
            self.proc.close()
        except Exception:
            pass
        return True

    def suspend(self):
        try:
            pid = self.proc.processId()
            if pid == 0:
                return False
            kernel32 = ctypes.windll.kernel32
            PROCESS_SUSPEND_RESUME = 0x0800
            handle = kernel32.OpenProcess(PROCESS_SUSPEND_RESUME, False, pid)
            if not handle:
                return False
            ctypes.windll.ntdll.NtSuspendProcess(handle)
            kernel32.CloseHandle(handle)
            return True
        except Exception:
            return False

    def resume(self):
        try:
            pid = self.proc.processId()
            if pid == 0:
                return False
            kernel32 = ctypes.windll.kernel32
            PROCESS_SUSPEND_RESUME = 0x0800
            handle = kernel32.OpenProcess(PROCESS_SUSPEND_RESUME, False, pid)
            if not handle:
                return False
            ctypes.windll.ntdll.NtResumeProcess(handle)
            kernel32.CloseHandle(handle)
            return True
        except Exception:
            return False


# ═══════════════════════════════════════════════════════════════════════════
# Helpers
# ═══════════════════════════════════════════════════════════════════════════
def human_sz(sz):
    for u in ["Б","КБ","МБ","ГБ","ТБ"]:
        if sz<1024: return f"{sz:.0f} {u}"
        sz/=1024
    return f"{sz:.1f} ПБ"

def normalize_snap_path(path: str) -> str:
    raw = (path or "").replace("\\", "/").strip()
    if not raw:
        return "/"
    if raw == "/":
        return raw
    if not raw.startswith("/"):
        raw = "/" + raw
    return raw.rstrip("/") or "/"

def compact_snap_paths(paths: List[str]) -> List[str]:
    cleaned = sorted(
        {normalize_snap_path(p) for p in paths if p},
        key=lambda p: (len(PurePosixPath(p).parts), p.lower()),
    )
    result: List[str] = []
    for path in cleaned:
        covered = any(
            path == existing or path.startswith(existing.rstrip("/") + "/")
            for existing in result
        )
        if not covered:
            result.append(path)
    return result

def restore_output_path(target_root: str, snap_path: str) -> Path:
    out = Path(target_root)
    for part in PurePosixPath(normalize_snap_path(snap_path)).parts:
        if part not in ("/", ""):
            out /= part
    return out

def tg_send(token,chat,msg,thread_id=None):
    if not _HAS_REQUESTS or not token or not chat: return
    try:
        data={"chat_id":chat,"text":msg}
        if thread_id and str(thread_id).strip():
            data["message_thread_id"]=str(thread_id).strip()
        _req.post(f"https://api.telegram.org/bot{token}/sendMessage",data=data,timeout=10)
    except: pass

def tg_broadcast(token,chat,group,msg):
    if group and str(group).strip():
        tg_send(token,chat,msg,thread_id=group)
    else:
        tg_send(token,chat,msg)

def tg_test(token,chat,thread_id=None):
    if not _HAS_REQUESTS or not token or not chat:
        return (False, "не задан токен или chat_id")
    try:
        data={"chat_id":chat,"text":"✅ RestLex тест"}
        if thread_id and str(thread_id).strip():
            data["message_thread_id"]=str(thread_id).strip()
        r=_req.post(f"https://api.telegram.org/bot{token}/sendMessage",data=data,timeout=10)
        resp = r.json()
        ok = r.status_code == 200 and resp.get("ok", False)
        if ok:
            return (True, "")
        desc = resp.get("description", "")
        return (False, desc or f"HTTP {r.status_code}")
    except Exception as e:
        return (False, str(e))

def _http_get_code(url, headers=None, timeout=8):
    """GET URL → HTTP-код (4xx/5xx тоже, без исключения). Для тестов соединения."""
    headers = headers or {}
    if _HAS_REQUESTS:
        return _req.get(url, headers=headers, timeout=timeout).status_code
    req = urllib.request.Request(url, headers=headers, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return getattr(r, "status", 200)
    except urllib.error.HTTPError as e:
        return e.code

def _http_post_code(url, body, headers=None, timeout=8):
    """POST body → HTTP-код (4xx/5xx тоже, без исключения). Для тестов соединения."""
    headers = headers or {}
    if _HAS_REQUESTS:
        return _req.post(url, data=body, headers=headers, timeout=timeout).status_code
    req = urllib.request.Request(url, data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return getattr(r, "status", 200)
    except urllib.error.HTTPError as e:
        return e.code

def _normalize_report_url(url: str) -> str:
    """Гарантирует что URL оканчивается на /report."""
    if not url:
        return url
    stripped = url.rstrip("/")
    if not stripped.endswith("/report"):
        stripped += "/report"
    return stripped

def web_report_send(url: str, report: dict, api_key: str = "") -> bool:
    if not url:
        return True
    url = _normalize_report_url(url)
    try:
        headers = {"Content-Type": "application/json; charset=utf-8"}
        if api_key:
            headers["X-Api-Key"] = api_key
        if _HAS_REQUESTS:
            r = _req.post(url, json=report, headers=headers, timeout=15)
            return 200 <= r.status_code < 300
        body = json.dumps(report, ensure_ascii=False).encode("utf-8")
        req = urllib.request.Request(url, data=body, headers=headers, method="POST")
        with urllib.request.urlopen(req, timeout=15) as resp:
            return 200 <= getattr(resp, "status", 200) < 300
    except Exception:
        return False

def detect_snapshot_id(lines: List[str]) -> Optional[str]:
    for line in reversed(lines):
        m = re.search(r"\bsnapshot\s+([0-9a-f]{6,64})\b", line, re.IGNORECASE)
        if m:
            return m.group(1)
    return None

def _fmt_bytes_ui(n: int) -> str:
    v = float(n)
    for u in ("Б","КБ","МБ","ГБ","ТБ"):
        if v < 1024.0: return f"{v:.1f} {u}"
        v /= 1024.0
    return f"{v:.1f} ПБ"

def parse_restic_summary(lines: List[str]) -> dict:
    """Parse restic JSON summary line from backup output."""
    for line in reversed(lines):
        stripped = line.strip()
        if stripped.startswith("{") and "message_type" in stripped and "summary" in stripped:
            try:
                d = json.loads(stripped)
                if d.get("message_type") == "summary":
                    return d
            except Exception:
                continue
    return {}

def _agent_ver_str() -> str:
    base = Path(r"C:\ProgramData\RestLex")
    def _r(name: str) -> str:
        try:
            return (base / name).read_text(encoding="utf-8").strip() or "?"
        except Exception:
            return "?"
    return f"{_r('launcher.version')}/{_r('agent.version')}"

def build_backup_report(
    profile_name: str,
    pid: str,
    exit_code: int,
    started_at: Optional[datetime],
    lines: List[str],
    agent_version: str,
    source_paths: Optional[List[str]] = None,
    dest_path: str = "",
) -> dict:
    started = started_at or datetime.now()
    duration = max(0, int((datetime.now() - started).total_seconds()))
    errors = []
    max_error_count = 0
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("{"):
            try:
                d = json.loads(stripped)
                mt = d.get("message_type", "")
                if mt == "error" and d.get("error", {}).get("message"):
                    errors.append(d["error"]["message"])
                elif mt == "status" and d.get("error_count", 0) > max_error_count:
                    max_error_count = d["error_count"]
            except Exception:
                pass
        elif (line.startswith("ERR:") or line.startswith("❌")
              or "ошиб" in line.lower() or "error" in line.lower()
              or "fatal" in line.lower()):
            errors.append(line)
    if not errors and max_error_count > 0:
        errors.append(json.dumps({"error_count": max_error_count}))
    snapshot_id = detect_snapshot_id(lines)
    # Determine status
    if exit_code == 0:
        status = "success"
        operation_status = "Backup_Success"
    elif exit_code == 130:
        # restic >= 0.19.0 отдаёт 130 на SIGINT (раньше 1). Это прерывание, а не сбой —
        # иначе штатная остановка улетает в Telegram как Backup_Failed.
        status = "error"
        operation_status = "Backup_Cancelled"
    elif exit_code == 3:
        # Backup completed but some files had errors
        status = "error"
        operation_status = "Backup_Error"
    else:
        # Backup failed completely
        status = "failed"
        operation_status = "Backup_Failed"
        if not errors:
            errors = lines[-10:]
    # Parse restic summary for sizes
    summary = parse_restic_summary(lines)
    files_new = summary.get("files_new", 0) or 0
    files_changed = summary.get("files_changed", 0) or 0
    bytes_added = summary.get("data_added", 0) or 0
    total_bytes_processed = summary.get("total_bytes_processed", 0) or 0
    return {
        "profile": profile_name,
        "pid": pid,
        "status": status,
        "operation_status": operation_status,
        "snapshot_id": snapshot_id,
        "started_at": started.isoformat(),
        "duration_s": duration,
        "files_new": files_new,
        "files_changed": files_changed,
        "bytes_added": bytes_added,
        "total_bytes_processed": total_bytes_processed,
        "errors": errors[:10],
        "source_paths": source_paths or [],
        "dest_path": dest_path,
        "host": os.environ.get("COMPUTERNAME", "unknown"),
        "agent_version": agent_version,
    }

def app_dir() -> Path:
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parent

def hidden_console_subprocess_kwargs() -> dict:
    if os.name != "nt":
        return {}
    info = subprocess.STARTUPINFO()
    info.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    info.wShowWindow = 0
    return {
        "creationflags": 0x08000000,
        "startupinfo": info,
    }

def normalize_mac(s: str) -> str:
    """MAC в любом виде (AA:BB.., aa-bb.., aabb.ccdd.eeff, через пробелы, слитно)
    → 'AA:BB:CC:DD:EE:FF'; '' если это не 12 hex-цифр."""
    h = re.sub(r"[\s:\-.]", "", s or "")
    if not re.fullmatch(r"[0-9A-Fa-f]{12}", h):
        return ""
    return ":".join(h[i:i + 2] for i in range(0, 12, 2)).upper()

def mac_to_ip(mac: str) -> str:
    """IP по MAC из ARP-кэша ('' если нет). ponytail: только кэш, без скана подсети —
    хост должен быть в той же сети и недавно общаться с этим ПК."""
    m = normalize_mac(mac).replace(":", "-").lower()
    if not m:
        return ""
    try:
        out = subprocess.run(["arp", "-a"], capture_output=True, text=True, errors="replace",
                             timeout=10, **hidden_console_subprocess_kwargs()).stdout
    except Exception:
        return ""
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].lower() == m:
            return parts[0]
    return ""

def send_wol(mac: str, host: str = "") -> bool:
    """Magic packet на 255.255.255.255 и (если host — IPv4) на его /24 broadcast,
    порты 9 и 7, с каждого локального IPv4: на машине с несколькими сетевыми
    255.255.255.255 уходит только в один интерфейс."""
    h = normalize_mac(mac).replace(":", "")
    if not h:
        return False
    pkt = bytes.fromhex("FF" * 6 + h * 16)
    targets = {"255.255.255.255"}
    if re.fullmatch(r"\d{1,3}(\.\d{1,3}){3}", host or ""):
        targets.add(host.rsplit(".", 1)[0] + ".255")
    try:
        local = {ai[4][0] for ai in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET)}
    except OSError:
        local = set()
    sent = False
    for src in local | {""}:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
                s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
                s.bind((src, 0))
                for t in targets:
                    for port in (9, 7):
                        s.sendto(pkt, (t, port))
                        sent = True
        except OSError:
            pass
    return sent

def open_ping_window(target: str):
    """Отдельное консольное окно `ping -t target` (закрывается пользователем)."""
    if os.name == "nt" and re.fullmatch(r"[\w.\-:]+", target or ""):
        subprocess.Popen(["ping", "-t", target], creationflags=subprocess.CREATE_NEW_CONSOLE)

def resolve_power_target(mac: str, prof: dict) -> str:
    """Куда слать shutdown /m: IP по MAC из ARP, иначе wol_host, если MAC тот же, что в WOL."""
    ip = mac_to_ip(mac)
    if not ip and normalize_mac(mac) and normalize_mac(mac) == normalize_mac(prof.get("wol_mac", "")):
        ip = str(prof.get("wol_host", "") or "").strip()
    return ip

def reverse_mac(mac: str) -> str:
    """MAC задом наперёд — адрес пакета Sleep-on-LAN (формат утилиты SleepOnLan)."""
    m = normalize_mac(mac)
    return ":".join(reversed(m.split(":"))) if m else ""

def send_sleep(mac: str, host: str = "") -> bool:
    """Пакет выключения: magic packet с перевёрнутым MAC. Сетевая карта сама ПК не
    выключит — пакет ловит restlex-agent на том ПК (галка «Разрешить выключать по сети»)."""
    return send_wol(reverse_mac(mac), host)

SLEEP_ON_LAN_DELAY = 30   # таймер выключения на принимающем ПК (зашит в агенте)
SLEEP_ON_LAN_CHECK = 120  # через сколько сек после отправки пробуем ping IP

def sleep_on_lan_probe_message(profile: str, mac: str, ip: str, reachable: bool) -> str:
    """Ping IP не доказывает ни доставку UDP, ни состояние устройства с MAC."""
    observation = ("отвечает на ping" if reachable else "не отвечает на ping")
    return (f"RestLex - Профиль: {profile} | Sleep-on-LAN: через {SLEEP_ON_LAN_CHECK} с "
            f"адрес {ip} {observation}. Состояние ПК {mac} и доставка пакета "
            f"не подтверждены: IP/ARP может указывать на другой ПК.")

# ═══════════════════════════════════════════════════════════════════════════
# BackupHeartbeat (Layer 2) — пишем в ProgramData\RestLex\active-backups\
# <repo_hash>.json с {pid, host, repo, started, profile}, обновляем mtime
# раз в минуту, удаляем в finally. Свой источник истины ещё до restic-лока.
# ═══════════════════════════════════════════════════════════════════════════
class BackupHeartbeat:
    DIR = PROGRAMDATA_ROOT / "RestLex" / "active-backups"

    def __init__(self, repo: str, profile_name: str = ""):
        self.repo = repo
        self.profile = profile_name
        h = hashlib.sha1(repo.encode("utf-8", errors="replace")).hexdigest()[:16]
        self.path = self.DIR / f"{h}.json"
        self._stop = threading.Event()
        self._write_lock = threading.Lock()
        self._thread = None
        self._started_iso = ""

    def _write(self, status: str, extra: dict = None):
        data = {
            "pid": os.getpid(),
            "host": socket.gethostname(),
            "repo": self.repo,
            "profile": self.profile,
            "status": status,
            "started": self._started_iso,
            "last_seen": datetime.now().isoformat(timespec="seconds"),
        }
        if extra:
            data.update(extra)
        with self._write_lock:
            if self._stop.is_set():
                return
            self.path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")

    def start(self):
        self._stop = threading.Event()
        try:
            self.DIR.mkdir(parents=True, exist_ok=True)
            self._started_iso = datetime.now().isoformat(timespec="seconds")
            self._write("running")
        except Exception:
            return
        self._thread = threading.Thread(target=self._loop, daemon=True)
        self._thread.start()

    def _loop(self):
        while not self._stop.wait(60):
            try:
                self._write("running")
            except Exception:
                pass

    def stop(self, status: str = "completed"):
        self._stop.set()
        # Не даём фоновому потоку заново создать heartbeat после удаления.
        # _write проверяет event под тем же lock.
        with self._write_lock:
            try:
                if self.path.exists():
                    self.path.unlink()
            except Exception:
                pass


# ═══════════════════════════════════════════════════════════════════════════
# LockPreflight (Layer 1) — анализ restic-локов и принятие решений:
# PROCEED | UNLOCK_FORCE | ABORT | WAIT.
# Asynchronous, делает работу в фоновом потоке, эмитит done(code, message).
# ═══════════════════════════════════════════════════════════════════════════
class LockPreflight(QObject):
    done = pyqtSignal(int, str)  # 0=OK, 1=ABORT(активный бэкап), 2=WAIT, 99=error

    def __init__(self, restic: str, repo: str, password: str,
                 grace_minutes: int = 35, force_orphan: bool = True, parent=None):
        super().__init__(parent)
        self.restic = restic
        self.repo = repo
        self.password = password
        self.grace = max(1, int(grace_minutes))
        self.force_orphan = bool(force_orphan)
        self._cancelled = threading.Event()

    def cancel(self):
        self._cancelled.set()

    def run(self):
        threading.Thread(target=self._run_bg, daemon=True).start()

    def _env(self):
        e = os.environ.copy()
        e["RESTIC_REPOSITORY"] = self.repo
        e["RESTIC_PASSWORD"]   = self.password
        return e

    def _list_locks(self):
        try:
            r = subprocess.run(
                [self.restic, "list", "locks"],
                env=self._env(), capture_output=True, text=True, timeout=30,
                **hidden_console_subprocess_kwargs())
            if r.returncode != 0:
                return None
            return [ln.strip() for ln in (r.stdout or "").splitlines() if ln.strip()]
        except Exception:
            return None

    def _cat_lock(self, lock_id: str):
        try:
            r = subprocess.run(
                [self.restic, "cat", "lock", lock_id],
                env=self._env(), capture_output=True, text=True, timeout=15,
                **hidden_console_subprocess_kwargs())
            if r.returncode != 0:
                return None
            return json.loads(r.stdout)
        except Exception:
            return None

    def _unlock(self) -> int:
        args = [self.restic, "unlock"]
        try:
            r = subprocess.run(args, env=self._env(),
                capture_output=True, text=True, timeout=30,
                **hidden_console_subprocess_kwargs())
            return r.returncode
        except Exception:
            return 99

    @staticmethod
    def _pid_is_restic(pid: int) -> Optional[bool]:
        """True = работающий restic, False = PID отсутствует/занят другой программой.
        None = не удалось установить владельца PID; такой lock снимать нельзя.
        Используем WinAPI вместо локализованного вывода tasklist.
        """
        if pid <= 0:
            return None
        handle = None
        try:
            k = ctypes.windll.kernel32
            k.OpenProcess.argtypes = [ctypes.c_uint, ctypes.c_int, ctypes.c_uint]
            k.OpenProcess.restype = ctypes.c_void_p
            handle = k.OpenProcess(0x1000, 0, int(pid))  # PROCESS_QUERY_LIMITED_INFORMATION (Vista+)
            if not handle and k.GetLastError() == 87:
                # На XP недоступен PROCESS_QUERY_LIMITED_INFORMATION. Повторяем
                # со старым правом; при неизвестном API для имени — только WAIT.
                handle = k.OpenProcess(0x0400, 0, int(pid))  # PROCESS_QUERY_INFORMATION
            if not handle:
                return False if k.GetLastError() == 87 else None  # нет такого PID / неизвестная ошибка
            k.QueryFullProcessImageNameW.argtypes = [
                ctypes.c_void_p, ctypes.c_uint, ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_uint)]
            k.QueryFullProcessImageNameW.restype = ctypes.c_int
            buf = ctypes.create_unicode_buffer(32768)
            n = ctypes.c_uint(len(buf))
            if not k.QueryFullProcessImageNameW(handle, 0, buf, ctypes.byref(n)):
                return None
            exe = os.path.basename(buf.value.replace("/", "\\")).lower()
            return exe == "restic.exe"
        except Exception:
            return None
        finally:
            if handle:
                ctypes.windll.kernel32.CloseHandle(ctypes.c_void_p(handle))

    def _decide(self, info: dict, my_host: str):
        from datetime import timezone
        lock_host = str(info.get("hostname", "") or "")
        lock_pid  = int(info.get("pid", 0) or 0)
        lock_time = str(info.get("time", "") or "")
        try:
            ts = lock_time.replace("Z", "+00:00")
            lock_dt = datetime.fromisoformat(ts)
            now_dt  = datetime.now(timezone.utc) if lock_dt.tzinfo else datetime.now()
            age_min = (now_dt - lock_dt).total_seconds() / 60.0
        except Exception:
            age_min = 0.0
        same_host = lock_host.lower() == my_host.lower()
        if same_host:
            pid_is_restic = self._pid_is_restic(lock_pid)
            if pid_is_restic is True:
                return ("ABORT", f"активный бэкап PID {lock_pid} на этом ПК")
            if pid_is_restic is None:
                return ("WAIT", f"не удалось проверить PID {lock_pid}; lock не снимаем")
            return ("UNLOCK_FORCE", f"лок без активного restic (PID {lock_pid}, возраст {age_min:.1f} мин)")
        # Без удалённого подтверждения возраста lock недостаточно: другой ПК
        # может долго работать, а часы могут расходиться. Не удаляем его lock.
        return ("WAIT", f"lock другого ПК {lock_host or 'неизвестен'} (возраст {age_min:.1f} мин): требуется проверка владельца")

    def _run_bg(self):
        try:
            locks = self._list_locks()
            if locks is None:
                # Не маскируем ошибку чтения lock попыткой удалить их.
                self.done.emit(99, "Не удалось прочитать locks: снятие блокировок запрещено")
                return
            if not locks:
                self.done.emit(0, "локов нет")
                return
            my_host = socket.gethostname()
            decisions = []
            for lid in locks:
                info = self._cat_lock(lid)
                if not isinstance(info, dict):
                    self.done.emit(99, f"Не удалось прочитать lock {lid}: автоматическое снятие запрещено")
                    return
                decisions.append((lid, info, self._decide(info, my_host)))
            actions = [d[2][0] for d in decisions]
            if "ABORT" in actions:
                reasons = "; ".join(d[2][1] for d in decisions if d[2][0] == "ABORT")
                self.done.emit(1, reasons)
                return
            if "WAIT" in actions and not all(a == "PROCEED" for a in actions):
                reasons = "; ".join(d[2][1] for d in decisions if d[2][0] == "WAIT")
                self.done.emit(2, reasons)
                return
            # все UNLOCK_FORCE (и/или PROCEED)
            need_unlock = any(a == "UNLOCK_FORCE" for a in actions)
            if need_unlock:
                if not self.force_orphan:
                    self.done.emit(2, "orphan-локи присутствуют, force_unlock_orphan=off")
                    return
                # Повторная сверка защищает от появления новых locks во время
                # анализа; у restic --remove-all нет атомарного CAS.
                fresh = self._list_locks()
                if self._cancelled.is_set():
                    self.done.emit(2, "Проверка блокировок отменена")
                    return
                if fresh is None or set(fresh) != set(locks):
                    self.done.emit(2, "Набор locks изменился: unlock отменён")
                    return
                # Обычный restic unlock сам проверяет stale-состояние в момент
                # удаления. --remove-all мог бы снести новый живой lock,
                # появившийся сразу после повторного list.
                rc = self._unlock()
                if rc == 0:
                    remaining = self._list_locks()
                    if remaining is None or remaining:
                        self.done.emit(2, "После unlock остались locks; бэкап отложен")
                        return
                    if self._cancelled.is_set():
                        self.done.emit(2, "Проверка блокировок отменена")
                        return
                    detail = "; ".join(d[2][1] for d in decisions if d[2][0] == "UNLOCK_FORCE")
                    self.done.emit(0, f"сняты локи: {detail}")
                else:
                    self.done.emit(99, f"unlock rc={rc}")
                return
            self.done.emit(0, "PROCEED")
        except Exception as e:
            self.done.emit(99, str(e))


def local_agent_path() -> Path:
    return app_dir() / "restlex-agent.exe"

def rust_agent_source_path() -> Path:
    root = Path(__file__).resolve().parent
    local = root / "rust" / "target" / "release" / "restlex-agent.exe"
    if local.exists():
        return local
    return root.parent / "RESTLEX" / "rust" / "target" / "release" / "restlex-agent.exe"

def sync_local_agent_binary() -> Optional[Path]:
    dst = local_agent_path()
    src = rust_agent_source_path()
    if dst.exists() and (not src.exists() or dst.stat().st_mtime >= src.stat().st_mtime):
        return dst
    if not src.exists():
        return dst if dst.exists() else None
    dst.write_bytes(src.read_bytes())
    return dst

def run_agent_cmd(args: List[str]) -> str:
    exe = sync_local_agent_binary() or local_agent_path()
    if not exe.exists():
        raise FileNotFoundError(f"Не найден restlex-agent.exe: {exe}")
    kw = {
        "capture_output": True,
        "text": True,
        "encoding": "utf-8",
        "errors": "replace",
        "timeout": 30,
    }
    kw.update(hidden_console_subprocess_kwargs())
    r = subprocess.run([str(exe), *args], **kw)
    out = (r.stdout or "").strip()
    err = (r.stderr or "").strip()
    if r.returncode == 0:
        return out or "OK"
    raise RuntimeError((err + " " + out).strip() or f"Код {r.returncode}")

def agent_status_string() -> str:
    try:
        raw = run_agent_cmd(["status"])
    except Exception:
        return "Служба не установлена"
    if "не установлена" in raw.lower():
        return "Служба не установлена"
    low = raw.lower()
    if "running" in low:
        return "Запущена"
    if "stopped" in low or "pending" in low or "paused" in low:
        return "Остановлена"
    return raw

def _load_agent_port() -> int:
    """Read actual agent API port from api.port file (written by agent on startup)."""
    import os as _os
    port_file = _os.path.join(_os.environ.get("ProgramData", "C:\\ProgramData"), "restlex", "api.port")
    try:
        p = int(open(port_file).read().strip())
        if 17345 <= p <= 17354:
            return p
    except Exception:
        pass
    return 17345

# WebDAV-сервер агента работает БЕЗ Basic-аутентификации (см.
# _mount_connect_session). Флаг оставлен, чтобы вернуть Basic одной правкой,
# если у WebClient когда-нибудь поднимут BasicAuthLevel/включат SSL.
_MOUNT_BASIC_AUTH = False

_AGENT_HOST = "127.0.0.1"
_AGENT_PORT = _load_agent_port()
AGENT_API_BASE = f"http://127.0.0.1:{_AGENT_PORT}"

AGENT_TOKEN_MISSING_MSG = (
    "В config.json нет API-токена агента (api_token пуст).\n"
    "Агент генерирует его сам при старте службы — перезапустите службу "
    "RestlexAgent, после чего GUI сможет обращаться к её API."
)


_AGENT_TOKEN_TTL = 5.0                       # сек
_AGENT_TOKEN_CACHE: Optional[tuple] = None   # (stamp, monotonic, token)


def _agent_bearer_token() -> str:
    """Токен для REST API агента. Логика идентична v1_auth_middleware в агенте:
    api_token, а если он пуст — update_api_key.

    НИКАКИХ встроенных дефолтов: Config.api_token/update_api_key подставляют
    _UPD_API_KEY_DEFAULT (ключ update-сервера), которого агент не знает — он
    отвечал 403 на всё, и GUI рисовал «Служба не запущена» при живой службе.
    Читаем ТОЛЬКО то, что реально лежит в config.json (и в CLI/headless тоже),
    расшифровывая enc: — при encrypt_secrets=true токен на диске зашифрован.
    Файл предпочитаем снимку в памяти: агент дописывает api_token при старте
    службы, уже после того как GUI прочитал конфиг.

    Результат кешируется на _AGENT_TOKEN_TTL секунд с инвалидацией по mtime:
    функция зовётся на КАЖДЫЙ запрос к агенту, а поллинг идёт раз в секунды —
    читать и дешифровать config.json столько же раз незачем."""
    global _AGENT_TOKEN_CACHE
    try:
        st = Config.CFG.stat()
        stamp = (st.st_mtime_ns, st.st_size)
    except Exception:
        stamp = None
    now = time.monotonic()
    if _AGENT_TOKEN_CACHE is not None:
        c_stamp, c_at, c_tok = _AGENT_TOKEN_CACHE
        if c_stamp == stamp and (now - c_at) < _AGENT_TOKEN_TTL:
            return c_tok
    tok = _agent_bearer_token_uncached()
    _AGENT_TOKEN_CACHE = (stamp, now, tok)
    return tok


def _agent_bearer_token_uncached() -> str:
    for src in (Config.CFG, None):
        try:
            if src is not None:
                if not src.exists():
                    continue
                d = json.loads(src.read_text(encoding="utf-8"))
            else:
                cfg = _AGENT_TOKEN_CFG
                if cfg is None:
                    continue
                d = cfg._g
            if not isinstance(d, dict):
                continue
            for key in ("api_token", "update_api_key"):
                v = str(_dpapi_decrypt(str(d.get(key) or "")) or "").strip()
                # _dpapi_decrypt при неудаче отдаёт строку КАК ЕСТЬ, т.е. с
                # префиксом enc:. Отправить такое Bearer'ом — гарантированный 401
                # и при этом молчание вместо внятного AGENT_TOKEN_MISSING_MSG.
                # Отбрасываем: пусть сработает fallback и сообщение об отсутствии.
                if v and not v.startswith("enc:"):
                    return v
        except Exception:
            continue
    return ""


def _agent_auth_headers() -> dict:
    """Легаси-роуты агента (/status, /log, /run/:pid, /mount/*, /reload,
    /last-run, /rest-server/*, /update/*) закрыты тем же auth-middleware, что и
    /api/v1 — без Bearer они отвечают 401. Открыт только GET /health."""
    tok = _agent_bearer_token()
    return {"Authorization": f"Bearer {tok}"} if tok else {}


def agent_api_request(path: str, method="GET", payload=None, timeout=10):
    rel = path if path.startswith("/") else f"/{path}"
    url = AGENT_API_BASE + rel
    auth = _agent_auth_headers()

    def _explain(err: str) -> str:
        # Без токена агент отвечает 401/403 на всё — не оставляем пользователя
        # с голым «HTTP 403», из которого причина не видна.
        low = (err or "").lower()
        if not auth and ("401" in low or "403" in low
                         or "unauthor" in low or "forbidden" in low):
            return f"{err}\n\n{AGENT_TOKEN_MISSING_MSG}"
        return err

    try:
        if _HAS_REQUESTS:
            r = _req.request(method, url, json=payload, timeout=timeout, headers=auth or None)
            txt = (r.text or "").strip()
            if not (200 <= r.status_code < 300):
                raise RuntimeError(txt or f"HTTP {r.status_code}")
            ctype = (r.headers.get("Content-Type") or "").lower()
            if "json" in ctype and txt:
                return r.json()
            return txt
        body = None
        headers = dict(auth)
        if payload is not None:
            body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
            headers["Content-Type"] = "application/json; charset=utf-8"
        req = urllib.request.Request(url, data=body, headers=headers, method=method)
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            txt = resp.read().decode("utf-8", errors="replace").strip()
            ctype = (resp.headers.get("Content-Type") or "").lower()
            if "json" in ctype and txt:
                return json.loads(txt)
            return txt
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="replace").strip()
        msg = detail or f"HTTP {e.code}"
        if not auth and e.code in (401, 403):
            msg = f"{msg}\n\n{AGENT_TOKEN_MISSING_MSG}"
        raise RuntimeError(msg) from e
    except Exception as e:
        raise RuntimeError(_explain(str(e))) from e

def agent_api_health() -> bool:
    try:
        return str(agent_api_request("/health", timeout=3)).strip().lower() == "ok"
    except Exception:
        return False

REPO_TYPES = [
    ("local","💾 Локальная папка",True,
     "Путь к локальной папке или UNC-пути.",
     r"D:\backups\myrepo", r"^[A-Za-z]:[\\\/]|^\\\\"),
    ("rest","🌐 REST Server",False,
     "Введите хост:порт, путь, логин и пароль — URL собирается автоматически.\nФормат: rest:http://user:pass@host:port/path",
     "",""),
    ("sftp","🌐 SFTP",False,
     "Формат: sftp:user@host:/path/to/repo",
     "sftp:user@host:/path","^sftp:"),
    ("s3","☁ S3 / MinIO",False,
     "Формат: s3:https://endpoint/bucket/path",
     "s3:https://s3.amazonaws.com/bucket/repo","^s3:"),
    ("b2","☁ Backblaze B2",False,
     "Формат: b2:bucket:path","b2:mybucket:repo","^b2:"),
    ("azure","☁ Azure Blob",False,
     "Формат: azure:container:path","azure:mycontainer:repo","^azure:"),
    ("gs","☁ Google Cloud",False,
     "Формат: gs:bucket:path","gs:mybucket:repo","^gs:"),
    ("rclone","🔄 Rclone",False,
     "Формат: rclone:remote:path","rclone:myremote:repo","^rclone:"),
]
RIDX = {r[0]:i for i,r in enumerate(REPO_TYPES)}

# ═══════════════════════════════════════════════════════════════════════════
# Drag/Drop trees
# ═══════════════════════════════════════════════════════════════════════════
class DropTree(QTreeWidget):
    dropped = pyqtSignal(list, str)
    def __init__(self,p=None):
        super().__init__(p); self.setAcceptDrops(True)
    def dragEnterEvent(self,e):
        (e.acceptProposedAction() if e.mimeData().hasFormat("x-restlex") else e.ignore())
    def dragMoveEvent(self,e):
        (e.acceptProposedAction() if e.mimeData().hasFormat("x-restlex") else e.ignore())
    def dropEvent(self,e):
        if not e.mimeData().hasFormat("x-restlex"): e.ignore(); return
        paths=json.loads(bytes(e.mimeData().data("x-restlex")).decode())
        item=self.itemAt(e.pos())
        target=""
        if item:
            t=Path(item.data(0,Qt.UserRole) or "")
            target=str(t.parent if t.is_file() else t)
        self.dropped.emit(paths,target); e.acceptProposedAction()

class DragTree(QTreeWidget):
    def __init__(self,p=None):
        super().__init__(p); self.setDragEnabled(True)
        self.setDragDropMode(QAbstractItemView.DragOnly)
    def startDrag(self,_):
        items=self.selectedItems()
        paths=compact_snap_paths([it.data(0,Qt.UserRole) for it in items if it.data(0,Qt.UserRole)])
        if not paths: return
        m=QMimeData(); m.setData("x-restlex",json.dumps(paths).encode())
        d=QDrag(self); d.setMimeData(m); d.exec_(Qt.CopyAction)

    def keyboardSearch(self, text):
        """Как в проводнике: набор букв переходит на папку/файл, имя которого начинается с них.
        Штатный поиск Qt сравнивает с текстом колонки, а там впереди значок 📁/📄 — не находил ничего."""
        now = time.monotonic()
        buf = getattr(self, "_ks_buf", "") if now - getattr(self, "_ks_t", 0.0) <= 1.0 else ""
        buf += text.lower()
        self._ks_buf, self._ks_t = buf, now
        rows = []
        idx = self.model().index(0, 0)
        while idx.isValid():                # только видимые строки (раскрытые ветки)
            rows.append(idx)
            idx = self.indexBelow(idx)
        if not rows:
            return
        cur = self.currentIndex()
        pos = rows.index(cur) if cur in rows else -1

        def find(b: str) -> bool:
            cycle = len(set(b)) == 1        # «kkk» — перебор по элементам на «k»
            needle = b[0] if cycle else b
            start = pos + 1 if (cycle or pos < 0) else pos
            for i in range(len(rows)):
                ix = rows[(start + i) % len(rows)]
                it = self.itemFromIndex(ix)
                name = (it.data(0, Qt.UserRole + 5) or "") if it else ""
                if name.lower().startswith(needle):
                    self.setCurrentIndex(ix)
                    self.scrollTo(ix)
                    return True
            return False

        if not find(buf) and len(buf) > 1:
            # «ц», затем «д»: по «цд» пусто — начинаем заново с последней буквы.
            self._ks_buf = text.lower()
            find(self._ks_buf)


# ═══════════════════════════════════════════════════════════════════════════
# Conflict dialog
# ═══════════════════════════════════════════════════════════════════════════
class EqualSplitter(QSplitter):
    """Делит область ровно пополам при любом размере окна, пока пользователь сам не сдвинул разделитель."""
    def __init__(self, orientation, parent=None):
        super().__init__(orientation, parent)
        self._user_moved = False
        self.splitterMoved.connect(lambda *_: setattr(self, "_user_moved", True))

    def _equalize(self):
        if not self._user_moved and self.count() == 2:
            half = max(1, (self.size().width() - self.handleWidth()) // 2)
            self.setSizes([half, half])

    def resizeEvent(self, e):
        super().resizeEvent(e)
        self._equalize()

    def showEvent(self, e):
        super().showEvent(e)
        self._equalize()


class BusyPanel(QWidget):
    """Единая панель «идёт работа» по центру области снимка: текст + бегущий прогресс-бар
    (+ «Отмена», когда операцию можно прервать). Пока показана — прячет `target`."""
    def __init__(self, target, parent=None):
        super().__init__(parent)
        self._target = target
        ly = QVBoxLayout(self); ly.setContentsMargins(0, 0, 0, 0); ly.setSpacing(12)
        self.label = QLabel("")
        self.label.setAlignment(Qt.AlignCenter)
        self.label.setWordWrap(True)
        self.label.setStyleSheet("color:#666;font-size:14px;font-weight:600;")
        self.bar = QProgressBar()
        self.bar.setRange(0, 0)
        self.bar.setTextVisible(False)
        self.bar.setFixedSize(320, 14)
        self.btn_cancel = QPushButton("Отмена")
        self.btn_cancel.setFixedSize(120, 30)
        self.btn_cancel.setStyleSheet("QPushButton{font-weight:600;color:#8c1d18;}")
        self.btn_cancel.hide()
        ly.addStretch(1)
        ly.addWidget(self.label)
        ly.addWidget(self.bar, 0, Qt.AlignHCenter)
        ly.addWidget(self.btn_cancel, 0, Qt.AlignHCenter)
        ly.addStretch(1)
        QWidget.setVisible(self, False)

    def setText(self, text: str):
        self.label.setText((text or "").lstrip("⏳ ").strip())

    def setVisible(self, v: bool):
        super().setVisible(v)
        if self._target is not None:
            self._target.setVisible(not v)

    def show(self): self.setVisible(True)
    def hide(self): self.setVisible(False)


class ConflictDlg(QDialog):
    REPLACE=1; REPLACE_ALL=2; SKIP=3; CANCEL=4
    def __init__(self,name,parent=None,more=0):
        super().__init__(parent); self.choice=self.CANCEL
        self.setWindowTitle("Файл уже существует")
        ly=QVBoxLayout(self)
        tail = f"\n  …и ещё {more}" if more > 0 else ""
        ly.addWidget(QLabel(f"Уже есть по месту хранения:\n\n  {name}{tail}\n\nЧто сделать?"))
        row=QHBoxLayout()
        for lbl,v in [("Заменить",1),("Заменить все",2),("Пропустить",3),("Отменить",4)]:
            b=QPushButton(lbl)
            b.clicked.connect(lambda _,val=v: (setattr(self,"choice",val), self.accept()))
            row.addWidget(b)
        ly.addLayout(row)


# ═══════════════════════════════════════════════════════════════════════════
# Scheduler
# ═══════════════════════════════════════════════════════════════════════════
class SchedulerManager:
    _BASE = PROGRAMDATA_ROOT / APP_DATA_DIRNAME

    @staticmethod
    def _stable_tname(pid: str) -> str:
        return f"Restlex\\Backup_{pid[:8]}"

    @staticmethod
    def _legacy_tname(pid: str, name: str) -> str:
        safe = re.sub(r'[\\/:*?"<>|]', '_', name)[:40]
        return f"Restlex\\{safe}_{pid[:8]}"

    @staticmethod
    def _candidate_task_names(pid: str, name: str) -> List[str]:
        names = [SchedulerManager._stable_tname(pid)]
        legacy = SchedulerManager._legacy_tname(pid, name)
        if legacy not in names:
            names.append(legacy)
        return names

    @staticmethod
    def _script(pid: str) -> Path:
        d = SchedulerManager._BASE / "tasks"; d.mkdir(parents=True, exist_ok=True)
        return d / f"{pid}.ps1"

    @staticmethod
    def _task_name(pid: str, task_id: str) -> str:
        safe_task_id = re.sub(r"[^0-9A-Za-z_-]", "", str(task_id or ""))[:16] or "task"
        return f"Restlex\\Backup_{pid[:8]}_{safe_task_id}"

    @staticmethod
    def _task_script(pid: str, task_id: str) -> Path:
        d = SchedulerManager._BASE / "tasks"; d.mkdir(parents=True, exist_ok=True)
        safe_task_id = re.sub(r"[^0-9A-Za-z_-]", "", str(task_id or ""))[:16] or "task"
        return d / f"{pid}_{safe_task_id}.ps1"

    @staticmethod
    def _task_log(pid: str, task_id: str) -> Path:
        d = SchedulerManager._BASE / "logs"; d.mkdir(parents=True, exist_ok=True)
        safe_task_id = re.sub(r"[^0-9A-Za-z_-]", "", str(task_id or ""))[:16] or "task"
        return d / f"{pid}_{safe_task_id}_sched.log"

    @staticmethod
    def _query_task(task_name: str) -> dict:
        r = subprocess.run(
            ["schtasks","/query","/tn",task_name,"/fo","CSV","/nh"],
            capture_output=True, timeout=10, encoding="cp866", errors="replace",
            **hidden_console_subprocess_kwargs())
        if r.returncode != 0:
            return {"exists": False, "task_name": task_name, "message": (r.stdout + r.stderr).strip()}
        try:
            import csv, io
            row = next(csv.reader(io.StringIO(r.stdout.strip())))
            return {
                "exists": True,
                "task_name": task_name,
                "next": row[1] if len(row) > 1 else "",
                "status": row[2] if len(row) > 2 else "",
                "raw": r.stdout.strip(),
            }
        except Exception:
            return {"exists": True, "task_name": task_name, "raw": r.stdout.strip()}

    @staticmethod
    def _make_ps1(restic, repo, password, sources, use_vss, retention, with_forget, log,
                  pid="", profile_name="", report_url="", tg_token="", tg_chat="",
                  temp="", excludes=None, tg_fatal=False, tg_file=False, tg_prune=False,
                  report_api_key="", tg_group="", vss_timeout_s=300) -> str:
        pw  = password.replace("'","''")
        tt  = tg_token.replace("'","''")
        tc  = tg_chat.replace("'","''")
        tg  = tg_group.replace("'","''")
        ru  = report_url.replace("'","''")
        rak = report_api_key.replace("'","''")
        pn  = profile_name.replace("'","''")
        pid = pid.replace("'","''")
        tmp = temp.replace("'","''")
        srcs = " ".join(f'"{s}"' for s in sources)
        vss  = f"--use-fs-snapshot -o vss.timeout={int(vss_timeout_s)}s" if use_vss else ""
        excl = " ".join(f'--exclude "{e.replace(chr(34), chr(34)+chr(34))}"' for e in (excludes or []))
        lines = [
            f"$r='{restic}'",
            f"$env:RESTIC_REPOSITORY='{repo}'",
            f"$env:RESTIC_PASSWORD='{pw}'",
        ]
        if tmp:
            lines += [
                f"$env:TMPDIR='{tmp}'",
                f"$env:TEMP='{tmp}'",
                f"$env:TMP='{tmp}'",
            ]
        lock_file = log.replace(".log", ".lock")
        lines += [
            f"$tg_token='{tt}'",
            f"$tg_chat='{tc}'",
            f"$tg_group='{tg}'",
            f"$report_url='{ru}'",
            f"$report_api_key='{rak}'",
            f"$profile_name='{pn}'",
            f"$profile_id='{pid}'",
            f"$log='{log}'",
            f"$lock='{lock_file}'",
            f"$tg_fatal=$({'$true' if tg_fatal else '$false'})",
            f"$tg_file=$({'$true' if tg_file else '$false'})",
            f"$tg_prune=$({'$true' if tg_prune else '$false'})",
            "if($report_url -ne '' -and -not $report_url.TrimEnd('/').EndsWith('/report')){$report_url=$report_url.TrimEnd('/')+ '/report'}",
            "# schema:v4",
            "$_lv=try{(Get-Content 'C:\\ProgramData\\RestLex\\launcher.version' -ErrorAction Stop).Trim()}catch{'?'}",
            "$_av=try{(Get-Content 'C:\\ProgramData\\RestLex\\agent.version' -ErrorAction Stop).Trim()}catch{'?'}",
            "$agent_ver_str=\"$_lv/$_av\"",
            "$main_log='C:\\ProgramData\\RestLex\\logs\\'+$profile_id+'.log'",
            "function wl($m){Add-Content $log \"[$(Get-Date -f 's')] $m\" -Enc UTF8}",
            "function wl_main($lv,$m){try{Add-Content $main_log \"[$(Get-Date -f 'yyyy-MM-dd HH:mm:ss')][$lv][scheduler] $m\" -Enc UTF8}catch{}}",
            "function tg_send($msg){",
            "  if($tg_token -eq '' -or $tg_chat -eq ''){return}",
            "  try{",
            "    $d=@{chat_id=$tg_chat;text=$msg;parse_mode='HTML'}",
            "    if($tg_group -ne ''){$d.message_thread_id=$tg_group}",
            "    $json=$d|ConvertTo-Json",
            "    $bytes=[System.Text.Encoding]::UTF8.GetBytes($json)",
            "    Invoke-RestMethod -Uri \"https://api.telegram.org/bot$tg_token/sendMessage\" -Method Post -ContentType 'application/json; charset=utf-8' -Body $bytes|Out-Null",
            "  }catch{}",
            "}",
            # Lock: prevent duplicate runs
            "if(Test-Path $lock){",
            "  wl 'SKIP: already running (lock exists)'",
            "  wl_main 'WARN' 'Запуск пропущен (уже выполняется)'",
            "  exit 0",
            "}",
            "New-Item -Path $lock -ItemType File -Force | Out-Null",
            "try {",
            "$started=Get-Date",
            "wl '=== backup start ==='",
            "wl_main 'INFO' \"Старт по расписанию: $profile_name\"",
            f"$o=& $r backup --json {vss} {excl} {srcs} 2>&1; $o|?{{$_ -notmatch '\"message_type\"\\s*:\\s*\"(status|verbose_status)\"'}}|%{{wl $_}}",
            "$bk_exit=$LASTEXITCODE",
            "wl \"exit $bk_exit\"",
            "if ($bk_exit -ne 0 -and $tg_token -ne '') {",
            "  $send=$false",
            "  if ($bk_exit -eq 1 -and $tg_fatal) { $send=$true }",
            "  if ($bk_exit -eq 3 -and $tg_file) { $send=$true }",
            "  if ($send) {",
            "    $tg_reason=if($bk_exit -eq 3){'Backup_Error (файлы)'}else{'Backup_Failed'}",
            "    tg_send \"RestLex - Профиль: $profile_name | Ошибка: $tg_reason\"",
            "  }",
            "}",
            # --- wl_main summary (same format as GUI main log) ---
            "$_snap=$null; $_fn=0; $_ba=0; $_dur_s=[int]((Get-Date)-$started).TotalSeconds; $_errs=@()",
            "foreach($_l in $o){",
            "  try{$_j=$_l|ConvertFrom-Json",
            "    if($_j.message_type -eq 'summary'){$_fn=$_j.files_new;$_ba=$_j.data_added;if($_j.snapshot_id){$_snap=$_j.snapshot_id}}",
            "    if($_j.message_type -eq 'error' -and $_j.error){$_errs+=$_j.error.message}",
            "  }catch{}",
            "  if($_l -match 'snapshot\\s+([0-9a-fA-F]{6,64})' -and -not $_snap){$_snap=$matches[1]}",
            "}",
            "if($bk_exit -eq 130){"
            "  wl_main 'CANCELLED' \"backup cancelled: exit=130 duration=${_dur_s}s\""
            "}elseif($bk_exit -eq 0){",
            "  wl_main 'SUCCESS' \"backup done: exit=0 files_new=$_fn bytes_added=$_ba duration=${_dur_s}s snapshot=$_snap\"",
            "}else{",
            "  wl_main 'FAILED' \"backup done: exit=$bk_exit files_new=$_fn bytes_added=$_ba duration=${_dur_s}s snapshot=$_snap\"",
            "  foreach($_e in ($_errs|Select-Object -First 5)){wl_main 'ERROR' $_e}",
            "}",
        ]
        lines += [
            "if ($report_url -ne '') {",
            "  try {",
            "    $snapshot_id=$null",
            "    $files_new=0; $files_changed=0; $bytes_added=0; $total_bytes=0",
            "    $err_lines=@(); $max_err_count=0",
            "    foreach($line in $o){",
            "      if($line -match 'snapshot\\s+([0-9a-fA-F]{6,64})'){ $snapshot_id=$matches[1] }",
            "      try{",
            "        $s=$line|ConvertFrom-Json",
            "        if($s.message_type -eq 'summary'){",
            "          $files_new=$s.files_new;$files_changed=$s.files_changed;$bytes_added=$s.data_added;$total_bytes=$s.total_bytes_processed",
            "          if($s.snapshot_id){$snapshot_id=$s.snapshot_id}",
            "        }",
            "        if($s.message_type -eq 'error' -and $s.error){$err_lines+=$s.error.message}",
            "        if($s.message_type -eq 'status' -and $s.error_count -gt $max_err_count){$max_err_count=$s.error_count}",
            "      }catch{}",
            "    }",
            "    $status=if($bk_exit -eq 0){'success'}elseif($bk_exit -eq 3){'error'}elseif($bk_exit -eq 130){'cancelled'}else{'failed'}",
            "    $op_status=if($bk_exit -eq 0){'Backup_Success'}elseif($bk_exit -eq 3){'Backup_Error'}elseif($bk_exit -eq 130){'Backup_Cancelled'}else{'Backup_Failed'}",
            "    $errors=@()",
            "    if($bk_exit -ne 0){",
            "      if($err_lines.Count -gt 0){ $errors=@($err_lines|Select-Object -Last 10) }",
            "      elseif($max_err_count -gt 0){ $errors=@(\"{`\"error_count`\":$max_err_count}\") }",
            "      else{ $errors=@($o|Where-Object{$_ -notmatch '^\\{' -and ($_ -match 'error|ошиб')}|Select-Object -Last 10) }",
            "      if($errors.Count -eq 0){ $errors=@($o|Where-Object{$_ -notmatch '^\\{'}|Select-Object -Last 10) }",
            "    }",
            "    $body=@{",
            "      profile=$profile_name;",
            "      pid=$profile_id;",
            "      status=$status;",
            "      operation_status=$op_status;",
            "      snapshot_id=$snapshot_id;",
            "      started_at=$started.ToString('o');",
            "      duration_s=[int]((Get-Date)-$started).TotalSeconds;",
            "      files_new=$files_new;",
            "      files_changed=$files_changed;",
            "      bytes_added=$bytes_added;",
            "      total_bytes_processed=$total_bytes;",
            "      errors=$errors;",
            "      source_paths=@(" + ",".join(f"'{s.replace(chr(39), chr(39)+chr(39))}'" for s in sources) + ");",
            f"      dest_path='{repo.replace(chr(39), chr(39)+chr(39))}';",
            "      host=$env:COMPUTERNAME;",
            "      agent_version=$agent_ver_str",
            "    } | ConvertTo-Json -Depth 4",
            "    $rh=@{'Content-Type'='application/json; charset=utf-8'}",
            "    if($report_api_key -ne ''){$rh['X-Api-Key']=$report_api_key}",
            "    $body_bytes=[System.Text.Encoding]::UTF8.GetBytes($body)",
            "    Invoke-RestMethod -Uri $report_url -Method Post -Headers $rh -Body $body_bytes | Out-Null",
            "  } catch { wl \"report error: $($_.Exception.Message)\" }",
            "}",
        ]
        if with_forget:
            flags = " ".join(f"--{k} {v}" for k,v in retention.items() if v)
            if flags:
                lines += [
                    "wl '=== forget/prune ==='",
                    f"$o=& $r forget --prune {flags} 2>&1; $o|%{{wl $_}}",
                    "$fr_exit=$LASTEXITCODE",
                    "wl \"forget exit $fr_exit\"",
                    "if ($fr_exit -ne 0 -and $tg_token -ne '' -and $tg_prune) {",
                    "  tg_send \"RestLex - Профиль: $profile_name | Ошибка: Prune_Failed\"",
                    "}",
                ]
        lines += [
            "wl '=== done ==='",
            "$_dur=[int]((Get-Date)-$started).TotalSeconds",
            "$_st=if($bk_exit -eq 0){'OK'}elseif($bk_exit -eq 3){'WARN'}elseif($bk_exit -eq 130){'CANCELLED'}else{'ERR'}",
            "wl_main $_st \"Завершено за ${_dur}s, exit=$bk_exit\"",
            "} finally {",
            "  Remove-Item -Path $lock -Force -ErrorAction SilentlyContinue",
            "}",
        ]
        return "\n".join(lines)

    # Маркер актуальной версии PS-скрипта.
    # При изменении формата bump → старые .ps1 будут пересозданы при старте GUI.
    PS_SCHEMA_MARKER = "# schema:v4"

    @staticmethod
    def migrate_outdated(cfg) -> int:
        """Регенерирует устаревшие .ps1 для всех schedule_tasks.
        Schtask указывает на .ps1 через /tr — менять задачу не требуется,
        достаточно перезаписать файл скрипта.
        Возвращает кол-во обновлённых задач."""
        count = 0
        marker = SchedulerManager.PS_SCHEMA_MARKER
        for pid, prof in cfg.profiles().items():
            for raw in (prof.get("sched_tasks", []) or []):
                task = raw if isinstance(raw, dict) else {}
                task_id = str(task.get("task_id") or "").strip()
                if not task_id:
                    continue
                ps1 = SchedulerManager._task_script(pid, task_id)
                try:
                    content = ps1.read_text(encoding="utf-8-sig") if ps1.exists() else ""
                except Exception:
                    content = ""
                if marker not in content:
                    try:
                        SchedulerManager.apply_task(cfg, pid, task)
                        count += 1
                    except Exception:
                        pass
        return count

    @staticmethod
    def apply(cfg, pid: str, sched: dict) -> tuple:
        # Phase 1: agent-scheduler handles execution — just save config, no schtask creation
        return True, "Сохранено в конфиге агента"

    @staticmethod
    def apply_task(cfg, pid: str, task: dict) -> tuple:
        # Phase 1: agent-scheduler handles execution — just save config, no schtask creation
        task_id = str(task.get("task_id") or "").strip()
        if not task_id:
            return False, "Не задан task_id"
        return True, "Сохранено в конфиге агента"

    @staticmethod
    def delete(pid: str, name: str) -> tuple:
        # Phase 1: no schtask to delete — clean up leftover PS1 file only
        try:
            SchedulerManager._script(pid).unlink()
        except OSError:
            pass
        return True, "Удалено"

    @staticmethod
    def delete_task(pid: str, task: dict) -> tuple:
        task_id = str(task.get("task_id") or "").strip()
        if not task_id:
            return False, "Не задан task_id"
        # Phase 1: no schtask to delete — clean up leftover PS1/log files only
        try:
            SchedulerManager._task_script(pid, task_id).unlink()
        except OSError:
            pass
        try:
            SchedulerManager._task_log(pid, task_id).unlink()
        except OSError:
            pass
        return True, "Удалено"

    @staticmethod
    def _compute_next_fire(entry: dict, last_run_ts: str = "") -> str:
        """Compute next expected fire time from schedule config."""
        import datetime as _dt
        freq = str(entry.get("freq", "daily") or "daily")
        times_raw = entry.get("times") or [entry.get("time", "02:00")]
        times = sorted(set(str(t) for t in times_raw if t)) or ["02:00"]
        now = _dt.datetime.now()
        if freq == "onlogon":
            return "При входе"
        if freq == "hourly":
            interval = max(1, int(entry.get("interval", 4) or 4))
            if last_run_ts:
                try:
                    last = _dt.datetime.fromisoformat(last_run_ts)
                    nxt = last + _dt.timedelta(hours=interval)
                    if nxt > now:
                        return nxt.strftime("%d.%m %H:%M")
                except Exception:
                    pass
            nxt = now + _dt.timedelta(hours=interval)
            return nxt.strftime("%d.%m %H:%M")
        if freq in ("daily", "weekly"):
            day_map = {"MON": 0, "TUE": 1, "WED": 2, "THU": 3, "FRI": 4, "SAT": 5, "SUN": 6}
            allowed_days = {day_map[d.strip().upper()] for d in str(entry.get("days", "MON") or "MON").split(",")
                            if d.strip().upper() in day_map}
            candidates = []
            for t_str in times:
                try:
                    h, m = map(int, t_str.split(":"))
                except Exception:
                    continue
                t = now.replace(hour=h, minute=m, second=0, microsecond=0)
                if t <= now:
                    t += _dt.timedelta(days=1)
                if freq == "weekly":
                    for delta in range(8):
                        candidate = t + _dt.timedelta(days=delta)
                        if candidate.weekday() in allowed_days and candidate > now:
                            candidates.append(candidate)
                            break
                else:
                    candidates.append(t)
            if candidates:
                return min(candidates).strftime("%d.%m %H:%M")
        return "—"

    # Кэш статуса агента: {running_now: [...], ts: float, online: bool}
    _status_cache: dict = {"running_now": [], "ts": 0.0, "refreshing": False, "online": False}
    # Кэш last-run per profile: pid -> {data: {}, ts: float, refreshing: bool}
    _last_run_cache: dict = {}

    @staticmethod
    def _refresh_status_cache():
        """Фоновое обновление кэша — не блокирует GUI."""
        import threading, time as _t
        if SchedulerManager._status_cache.get("refreshing"):
            return
        SchedulerManager._status_cache["refreshing"] = True
        def _do():
            try:
                d = agent_api_request("/status", timeout=2)
                SchedulerManager._status_cache["running_now"] = d.get("running_now") or []
                SchedulerManager._status_cache["ts"] = _t.monotonic()
                SchedulerManager._status_cache["online"] = True
            except Exception:
                SchedulerManager._status_cache["ts"] = _t.monotonic()
                SchedulerManager._status_cache["online"] = False
            finally:
                SchedulerManager._status_cache["refreshing"] = False
        threading.Thread(target=_do, daemon=True).start()

    @staticmethod
    def _agent_last_run_bg(pid: str):
        """Фоновое обновление кэша last-run для профиля."""
        import threading, time as _t
        cache = SchedulerManager._last_run_cache
        entry = cache.setdefault(pid, {"data": {}, "ts": 0.0, "refreshing": False})
        if entry.get("refreshing"):
            return
        entry["refreshing"] = True
        def _do():
            try:
                d = agent_api_request(f"/last-run/{pid}", timeout=3)
                cache[pid] = {"data": d if isinstance(d, dict) else {}, "ts": _t.monotonic(), "refreshing": False}
            except Exception:
                cache.setdefault(pid, {"data": {}, "ts": 0.0})["refreshing"] = False
                cache[pid]["ts"] = _t.monotonic()
        threading.Thread(target=_do, daemon=True).start()

    @staticmethod
    def _agent_is_running(pid: str) -> bool:
        import time as _t
        age = _t.monotonic() - SchedulerManager._status_cache["ts"]
        if age > 4.0:
            SchedulerManager._refresh_status_cache()
        return pid in SchedulerManager._status_cache["running_now"]

    @staticmethod
    def _agent_last_run(pid: str) -> dict:
        """Возвращает кэш last-run немедленно, обновляет в фоне если устарел."""
        import time as _t
        cache = SchedulerManager._last_run_cache
        entry = cache.get(pid, {})
        age = _t.monotonic() - entry.get("ts", 0.0)
        if age > 5.0:
            SchedulerManager._agent_last_run_bg(pid)
        return entry.get("data", {})

    @staticmethod
    def agent_online() -> bool:
        """True если агент отвечал при последней проверке (может быть устаревшим)."""
        ts = SchedulerManager._status_cache.get("ts", 0.0)
        if ts == 0.0:
            return True  # ещё не проверяли — не показываем предупреждение
        return SchedulerManager._status_cache.get("online", False)

    @staticmethod
    def status(pid: str, name: str) -> dict:
        # Phase 1: read last-run from agent instead of schtask status
        if SchedulerManager._agent_is_running(pid):
            return {
                "exists": True,
                "task_name": "agent-scheduler",
                "next": "—",
                "status": "⏳ Выполняется...",
                "last_run": {},
            }
        lr = SchedulerManager._agent_last_run(pid)
        status_str = lr.get("status", "")
        if status_str == "success":
            display = "✅ Успешно"
        elif status_str == "warning":
            display = "⚠️ Частичный (ошибки файлов)"
        elif status_str in ("failed", "error"):
            display = "❌ Ошибка"
        elif status_str == "crashed":
            display = "💥 Крэш агента"
        elif status_str:
            display = status_str
        else:
            display = "Нет данных"
        ts = lr.get("timestamp", "")
        if ts:
            try:
                import datetime as _dt
                dt = _dt.datetime.fromisoformat(ts)
                ts_label = dt.strftime("%d.%m %H:%M")
            except Exception:
                ts_label = ts[:16]
        else:
            ts_label = "—"
        return {
            "exists": True,
            "task_name": f"agent-scheduler",
            "next": "—",
            "status": f"{display} ({ts_label})" if ts_label != "—" else display,
            "last_run": lr,
        }

    @staticmethod
    def status_task(pid: str, task: dict) -> dict:
        # Phase 1: all tasks share the same agent last-run per profile
        task_id = str(task.get("task_id") or "").strip()
        result = SchedulerManager.status(pid, "")
        result["task_name"] = f"agent-scheduler / {task_id[:8]}" if task_id else "agent-scheduler"
        return result

    @staticmethod
    def run_now(pid: str, name: str) -> tuple:
        # Phase 1: trigger backup via agent API
        try:
            d = agent_api_request(f"/run/{pid}", method="POST", timeout=5)
            if isinstance(d, dict) and d.get("error"):
                return False, str(d["error"])
            return True, "Запуск инициирован"
        except Exception as e:
            return False, f"Агент недоступен: {e}"

    @staticmethod
    def run_task(pid: str, task: dict) -> tuple:
        # Phase 1: all tasks share the same profile — trigger via agent API
        return SchedulerManager.run_now(pid, "")


class RestServerManager:
    _BASE = PROGRAMDATA_ROOT / APP_DATA_DIRNAME

    @staticmethod
    def exe_path() -> Path:
        return RestServerManager._BASE / "bin" / "rest-server.exe"

    @staticmethod
    def htpasswd_path(data_dir: str) -> Path:
        return Path(data_dir) / ".htpasswd"

    @staticmethod
    def config_path() -> Path:
        return RestServerManager._BASE / "rest-server-config.json"

    @staticmethod
    def load_config() -> dict:
        p = RestServerManager.config_path()
        if p.exists():
            try:
                cfg = json.loads(p.read_text(encoding="utf-8"))
                if isinstance(cfg, dict) and isinstance(cfg.get("password"), str):
                    cfg["password"] = _dpapi_decrypt(cfg["password"])
                return cfg
            except Exception:
                pass
        return {"data_dir": "", "port": "8000", "username": "admin", "password": ""}

    @staticmethod
    def save_config(cfg: dict):
        # Пароль rest-server раньше писался плейнтекстом мимо DPAPI-слоя. Rust-агент
        # поле `password` из этого файла НЕ читает вообще (rest_server.rs::Config его
        # не объявляет, serde игнорирует неизвестный ключ), .htpasswd генерит только
        # GUI — поэтому enc: здесь безопасен и совместимость не ломается.
        out = dict(cfg)
        if _ENCRYPT_SECRETS and isinstance(out.get("password"), str):
            try:
                out["password"] = _dpapi_encrypt(out["password"])
            except _DpapiError as e:
                _config_warn(f"DPAPI недоступен ({e}) — пароль rest-server сохранён "
                             f"открытым текстом.")
        p = RestServerManager.config_path()
        p.parent.mkdir(parents=True, exist_ok=True)
        _atomic_write_json(p, out)

    @staticmethod
    def generate_htpasswd(data_dir: str, username: str, password: str) -> str:
        import base64
        sha = base64.b64encode(hashlib.sha1(password.encode("utf-8")).digest()).decode()
        content = f"{username}:{{SHA}}{sha}\n"
        htpasswd = RestServerManager.htpasswd_path(data_dir)
        htpasswd.parent.mkdir(parents=True, exist_ok=True)
        htpasswd.write_text(content, encoding="utf-8")
        return str(htpasswd)

    _AGENT_API = AGENT_API_BASE

    @staticmethod
    def _agent_call(method: str, path: str) -> dict:
        url = RestServerManager._AGENT_API + path
        headers = {"Content-Type": "application/json"}
        headers.update(_agent_auth_headers())   # /rest-server/* закрыт auth-middleware
        req = urllib.request.Request(url, method=method, headers=headers)
        if method == "POST":
            req.data = b"{}"
        with urllib.request.urlopen(req, timeout=5) as resp:
            return json.loads(resp.read().decode("utf-8"))

    @staticmethod
    def is_running() -> bool:
        try:
            data = RestServerManager._agent_call("GET", "/rest-server/status")
            return bool(data.get("running"))
        except Exception:
            return False

    @staticmethod
    def start(data_dir: str, port: str, htpasswd: str) -> tuple:
        cfg = RestServerManager.load_config()
        cfg.update({"enabled": True, "data_dir": data_dir, "port": port})
        RestServerManager.save_config(cfg)
        try:
            data = RestServerManager._agent_call("POST", "/rest-server/start")
            if data.get("ok"):
                return True, "rest-server запущен"
            return False, data.get("error", "Неизвестная ошибка агента")
        except Exception as e:
            return False, f"Агент недоступен ({e}). Убедитесь, что служба restlex-agent запущена."

    @staticmethod
    def stop() -> tuple:
        cfg = RestServerManager.load_config()
        cfg["enabled"] = False
        RestServerManager.save_config(cfg)
        try:
            RestServerManager._agent_call("POST", "/rest-server/stop")
        except Exception:
            pass
        return True, "rest-server остановлен"

    @staticmethod
    def fetch_latest_release() -> dict:
        api_url = "https://api.github.com/repos/restic/rest-server/releases/latest"
        gh_headers = {"User-Agent": "RestLex", "Accept": "application/vnd.github+json"}
        # requests+certifi: urllib в PyInstaller-сборке падает с CERTIFICATE_VERIFY_FAILED
        if _HAS_REQUESTS:
            resp = _req.get(api_url, headers=gh_headers, timeout=25)
            resp.raise_for_status()
            payload = resp.json()
        else:
            with urllib.request.urlopen(urllib.request.Request(api_url, headers=gh_headers),
                                        timeout=25) as resp:
                payload = json.loads(resp.read().decode("utf-8"))
        assets = payload.get("assets") or []
        preferred = ["windows_amd64.zip", "windows_386.zip", "windows_amd64.exe"]
        selected = None
        for suffix in preferred:
            selected = next((a for a in assets if str(a.get("name", "")).lower().endswith(suffix)), None)
            if selected:
                break
        if not selected:
            selected = next(
                (a for a in assets if "windows" in str(a.get("name", "")).lower()), None
            )
        if not selected:
            raise RuntimeError("В последнем релизе не найден файл rest-server для Windows")
        url = str(selected.get("browser_download_url") or "").strip()
        if not url:
            raise RuntimeError("GitHub не отдал ссылку на скачивание rest-server")
        return {
            "version": str(payload.get("tag_name") or "").lstrip("v"),
            "asset": selected,
            "url": url,
        }

    @staticmethod
    def download_exe() -> Path:
        release = RestServerManager.fetch_latest_release()
        target = RestServerManager.exe_path()
        target.parent.mkdir(parents=True, exist_ok=True)
        temp_dir = Path(tempfile.mkdtemp(prefix="restlex-rserver-"))
        try:
            asset_name = str(release["asset"].get("name") or "rest-server.zip")
            archive = temp_dir / asset_name
            if _HAS_REQUESTS:
                with _req.get(release["url"], headers={"User-Agent": "RestLex"}, timeout=120, stream=True) as resp:
                    resp.raise_for_status()
                    resp.raw.decode_content = True
                    with open(archive, "wb") as out:
                        shutil.copyfileobj(resp.raw, out)
            else:
                req = urllib.request.Request(release["url"], headers={"User-Agent": "RestLex"})
                with urllib.request.urlopen(req, timeout=120) as resp, open(archive, "wb") as out:
                    shutil.copyfileobj(resp, out)
            if archive.suffix.lower() == ".zip":
                with zipfile.ZipFile(archive, "r") as zf:
                    exe_name = next(
                        (n for n in zf.namelist()
                         if n.lower().endswith(".exe") and "rest" in Path(n).name.lower()),
                        next((n for n in zf.namelist() if n.lower().endswith(".exe")), "")
                    )
                    if not exe_name:
                        raise RuntimeError("В архиве не найден exe-файл rest-server")
                    with zf.open(exe_name) as src, open(temp_dir / "rest-server.exe", "wb") as dst:
                        shutil.copyfileobj(src, dst)
                    shutil.move(str(temp_dir / "rest-server.exe"), str(target))
            else:
                shutil.move(str(archive), str(target))
        finally:
            shutil.rmtree(temp_dir, ignore_errors=True)
        return target


class WebServerManager:
    _BASE = PROGRAMDATA_ROOT / APP_DATA_DIRNAME
    _CFG  = _BASE / "webserver-config.json"
    _DEFAULT: dict = {
        "host": "0.0.0.0", "port": 8798, "report_port": 8797,
        "admin_user": "admin", "admin_password": "admin",
        "viewer_password": "", "api_key": "",
        "history_count": 10, "auto_group": True,
        "stale_hours": 0, "tg_token": "", "tg_chat": "", "tg_group": "",
        "secret_key": "",
    }
    _AGENT_API = AGENT_API_BASE

    @classmethod
    def config_path(cls) -> Path:
        return cls._CFG

    @classmethod
    def load_config(cls) -> dict:
        p = cls.config_path()
        if p.exists():
            try:
                saved = json.loads(p.read_text(encoding="utf-8"))
                merged = {**cls._DEFAULT, **saved}
                # Веб-сервер (Rust) пишет свои секреты как enc: — расшифровываем,
                # иначе шифротекст попадал в поле «API-ключ» на вкладке Сервер, а
                # оттуда в cfg.report_api_key → агенты слали enc:… в X-Api-Key и
                # получали 401 на /report (отчёты о бэкапах переставали приходить).
                for k in _WEBSERVER_SECRET_KEYS:
                    if isinstance(merged.get(k), str):
                        merged[k] = _dpapi_decrypt(merged[k])
                return merged
            except Exception:
                pass
        return cls._DEFAULT.copy()

    @classmethod
    def save_config(cls, cfg: dict):
        # admin_password/viewer_password/api_key/tg_token/secret_key шифруются
        # DPAPI (machine-scope + base64, префикс enc:) при включённом тумблере
        # «Шифрование секретов». Формат байт-в-байт совместим с Rust:
        # restlex_core::crypto::decrypt_secret понимает и enc:, и плейнтекст, и
        # restlex-webserver теперь прогоняет через него эти поля.
        # НЕ возвращать «не шифруем, Rust не умеет» — умеет.
        import secrets as _sec
        out = dict(cfg)
        if not out.get("secret_key"):
            out["secret_key"] = _sec.token_hex(32)
        cfg["secret_key"] = out["secret_key"]   # вызывающий код читает cfg дальше
        # Шифруем БЕЗУСЛОВНО, не глядя на тумблер encrypt_secrets: Rust-вебсервер
        # пишет эти поля как enc: ВСЕГДА (web_server.rs), а load_config их всегда
        # расшифровывает. Если завязать шифрование на тумблер (дефолт False), то
        # любое сохранение вкладки «Веб-сервер» (в т.ч. авто-_wsvr_save_config при
        # переключении вкладок) ПОНИЖАЛО бы пароли/ключи до плейнтекста.
        # Тумблер управляет config.json/profiles.json, а не конфигом веб-сервера.
        for k in _WEBSERVER_SECRET_KEYS:
            v = out.get(k)
            if isinstance(v, str) and v:
                try:
                    out[k] = _dpapi_encrypt(v)
                except _DpapiError as e:
                    _config_warn(f"DPAPI недоступен ({e}) — секреты веб-сервера "
                                 f"сохранены открытым текстом.")
                    break
        p = cls.config_path()
        p.parent.mkdir(parents=True, exist_ok=True)
        _atomic_write_json(p, out)

    @classmethod
    def _agent_call(cls, method: str, path: str) -> dict:
        url = cls._AGENT_API + path
        headers = {"Content-Type": "application/json"}
        headers.update(_agent_auth_headers())   # легаси-роуты агента требуют Bearer
        req = urllib.request.Request(url, method=method, headers=headers)
        if method == "POST":
            req.data = b"{}"
        with urllib.request.urlopen(req, timeout=5) as resp:
            return json.loads(resp.read().decode("utf-8"))

    _SVC_NAME = "RestlexWebServer"

    @classmethod
    def _sc(cls, *args) -> "tuple[int, str]":
        try:
            kw = {"capture_output": True, "text": True, "timeout": 15}
            try:
                kw.update(hidden_console_subprocess_kwargs())
            except Exception:
                pass
            r = subprocess.run(["sc", *args], **kw)
            return r.returncode, (r.stdout or "") + (r.stderr or "")
        except Exception as e:
            return -1, str(e)

    @classmethod
    def is_running(cls) -> bool:
        rc, out = cls._sc("query", cls._SVC_NAME)
        return rc == 0 and "RUNNING" in out.upper()

    @classmethod
    def start(cls, cfg: dict) -> "tuple[bool, str]":
        cls.save_config(cfg)
        rc, out = cls._sc("start", cls._SVC_NAME)
        if rc == 0 or "уже запущена" in out.lower() or "already" in out.lower():
            return True, f"Веб-сервер запущен (порт {cfg.get('port', 8798)})"
        return False, f"Не удалось запустить службу {cls._SVC_NAME}: {out.strip() or 'служба не установлена?'}"

    @classmethod
    def stop(cls) -> "tuple[bool, str]":
        cls._sc("stop", cls._SVC_NAME)
        return True, "Веб-сервер остановлен"


# ═══════════════════════════════════════════════════════════════════════════
# Main Window
# ═══════════════════════════════════════════════════════════════════════════
class _ProfItemDelegate(QStyledItemDelegate):
    """Список профилей: имя слева, время запуска (UserRole+1) прижато вправо."""
    def paint(self, painter, option, index):
        o = QStyleOptionViewItem(option); self.initStyleOption(o, index)
        sched = str(index.data(Qt.UserRole + 1) or "")
        fm = o.fontMetrics
        r = o.rect.adjusted(4, 0, -6, 0)
        sw = fm.horizontalAdvance(sched) + 12 if sched else 0
        # имя рисует сам стиль (цвета выделения как у обычного списка), обрезав под время
        o.text = fm.elidedText(o.text, Qt.ElideRight, max(0, r.width() - sw - 6))
        o.widget.style().drawControl(QStyle.CE_ItemViewItem, o, painter, o.widget)
        if sched:
            # windowsvista рисует выделение светлым (тёмный текст); классический стиль
            # (сервер по RDP) — тёмно-синим с белым текстом
            sel = (bool(o.state & QStyle.State_Selected)
                   and o.widget.style().objectName().lower() != "windowsvista")
            painter.save()
            painter.setPen(o.palette.color(QPalette.HighlightedText) if sel
                           else QColor("#999" if index.data(Qt.UserRole + 2) else "#2a5db0"))
            painter.drawText(r, Qt.AlignVCenter | Qt.AlignRight, sched)
            painter.restore()


class RestlexUI(QMainWindow):
    _service_badge_signal = pyqtSignal(str)
    # Доставка callable в UI-поток из фоновых потоков. QTimer.singleShot(0, lambda)
    # из threading.Thread НЕ срабатывает (нет event loop в потоке) — колбэк молча
    # теряется. Queued-сигнал доставляется всегда.
    _ui_call_signal = pyqtSignal(object)

    def __init__(self, cli_run_backup_pid=None, cli_no_scan=False,
                 cli_check=False, cli_pre_cmd=""):
        super().__init__()
        self._service_badge_signal.connect(self._apply_service_badge)
        self._ui_call_signal.connect(lambda fn: fn())
        # CLI-флаги обязаны быть выставлены ДО _init_ui(): раньше main() присваивал
        # их после конструктора, а _init_ui проверял _cli_run_backup_pid внутри
        # __init__ — `restlex.exe --run-backup <pid>` просто открывал GUI.
        self._cli_run_backup_pid = cli_run_backup_pid
        self._cli_no_scan = cli_no_scan
        self._cli_check = cli_check
        self._cli_pre_cmd = cli_pre_cmd
        global _CONFIG_WARN_HOOK
        _CONFIG_WARN_HOOK = self._config_warn_ui
        self.cfg = Config()
        if self.cfg._read_failed:
            QMessageBox.warning(
                self, "Конфиг недоступен",
                "Не удалось прочитать файл(ы) конфигурации:\n\n"
                + "\n".join(self.cfg._read_failed)
                + "\n\nВероятно, файл был занят службой restlex-agent или нет прав "
                  "на чтение. Сейчас показаны значения по умолчанию; запись "
                  "отдельной настройки в этот момент будет отменена (чтобы не "
                  "затереть уже сохранённые параметры) — просто повторите "
                  "сохранение, когда файл освободится."
            )
        if self.cfg._corrupted:
            files = "\n".join(self.cfg._corrupted)
            QMessageBox.warning(
                self, "Повреждённый конфиг",
                f"Файл(ы) конфигурации повреждены (не читаются как JSON-объект). "
                f"Повреждённая копия отложена рядом с расширением .bad-<время>, "
                f"файл создан заново со значениями по умолчанию:\n\n{files}\n\n"
                f"Настройки можно вводить заново либо восстановить из .bad-копии."
            )
        try:
            lv = PROGRAMDATA_ROOT / APP_DATA_DIRNAME / "launcher.version"
            lv.parent.mkdir(parents=True, exist_ok=True)
            lv.write_text(APP_VERSION, encoding="utf-8")
        except Exception:
            pass
        # Auto-migrate outdated scheduler scripts (one-time per format bump)
        try:
            n = SchedulerManager.migrate_outdated(self.cfg)
            if n:
                print(f"[migrate] regenerated {n} outdated scheduler scripts")
        except Exception:
            pass
        self.pid: Optional[str] = None           # current profile id
        self._workers: Dict[str, ResticWorker] = {}
        self._snaps: List[dict] = []             # sorted snapshots list
        self._snap_workers: dict = {}
        self._ls_worker: Optional[ResticWorker] = None
        self._chk_worker: Optional[ResticWorker] = None
        self._chk_timer: Optional[QTimer] = None
        self._backup_timeout_timer: Optional[QTimer] = None
        self._res_monitor_timer: Optional[QTimer] = None
        self._chk_errs: List[str] = []
        self._replace_all = False
        self._cur_snap_id: Optional[str] = None
        self._snap_loading = False
        self._check_unknown_flag = False   # restic < 0.19 не знает "check --host"
        self._snap_refresh_pending = False
        self._snap_source_pid: Optional[str] = None
        self._snap_nodes_cache: Dict[str, List[dict]] = {}
        self._snapshots_worker: Optional[ResticWorker] = None
        self._snap_view_mode = "tree"
        self._snap_browser_unc_root = ""
        self._snap_browser_unc_target = ""
        self._snap_browser_last_target = ""
        self._snap_browser_hwnd = 0
        self._snap_browser_style = None
        self._snap_browser_exstyle = None
        self._shell_app = None
        self._restore_ui_mode = "old"
        self._restore_job: Optional[dict] = None
        self._post_backup_code: Optional[int] = None
        self._bk_started_at: Optional[datetime] = None
        self._bk_report_lines: List[str] = []
        self._logs_dirty = False
        self._loading_dialog: Optional[QProgressDialog] = None
        self._restore_progress_dialog: Optional[QDialog] = None
        self._restore_q: Optional[queue.Queue] = None
        self._restore_timer: Optional[QTimer] = None
        self._search_q: Optional[queue.Queue] = None
        self._search_poll_timer: Optional[QTimer] = None
        self._search_debounce: Optional[QTimer] = None
        self._search_proc: Optional[object] = None
        self._del_proc: Optional[object] = None
        self._del_gen = 0
        self._del_mode: Optional[str] = None
        self._deleted_scanned_folder = ""
        self._del_found = 0
        self._del_dir_mtime: Dict[str, str] = {}   # даты папок из потока ls — для строк-предков справа
        self._deleted_last = None          # (снимок, папка) последнего законченного сравнения
        self._del_nodes: Dict[str, QTreeWidgetItem] = {}   # путь снимка -> строка в правом дереве
        self._closing = False
        self._restore_in_progress = False
        self._sched_loading = False
        self._sched_loaded_key = None      # (pid, task_id) формы планировщика — чтобы
                                           # повторный выбор той же задачи не затирал правки
        self._repo_dirty = False
        self._tab_change_guard = False
        self._current_tab_widget = None
        self._local_path = Path("C:\\") if sys.platform=="win32" else Path("/")
        self._backup_running = False
        self._backup_pid = ""
        self._backup_paused = False
        self._repo_mutex: Optional[RepoMutex] = None
        self._repo_mutex_held = False
        self._heartbeat: Optional[BackupHeartbeat] = None
        self._bk_vss_ok: Optional[bool] = None
        self._log_queue: list = []
        self._init_ui()
        self._reload_profiles()
        self._ensure_restic_present()
        self._log_flush_timer = QTimer(self)
        self._log_flush_timer.setInterval(400)
        self._log_flush_timer.timeout.connect(self._flush_log_queue)
        self._log_flush_timer.start()
        self._bk_banner_timer = QTimer(self)
        self._bk_banner_timer.setInterval(5000)
        self._bk_banner_timer.timeout.connect(self._refresh_global_bk_banner)
        self._bk_banner_timer.start()
        QTimer.singleShot(300, self._refresh_global_bk_banner)
        QTimer.singleShot(1500, self._startup_agent_check)
        self._service_badge_timer = QTimer(self)
        self._service_badge_timer.setInterval(3000)
        self._service_badge_timer.timeout.connect(self._refresh_service_badge)
        self._service_badge_timer.start()
        QTimer.singleShot(300, self._refresh_service_badge)

    def _ensure_restic_present(self):
        managed = self.cfg.managed_restic_path()
        if managed.exists():
            return
        if shutil.which("restic"):
            return
        # Скачивание с GitHub (urlopen timeout=120) — только в фоне: раньше вызов
        # шёл прямо из __init__ и окно не рисовалось до 2 минут на первом запуске.
        # _restic_net_busy — общий флаг с кнопками «Проверить версию»/«Скачать
        # restic»: без него параллельный клик писал в тот же файл, а CLI-бэкап
        # стартовал раньше, чем докачается бинарник.
        if getattr(self, "_restic_net_busy", False):
            return
        self._restic_net_busy = True
        self.statusBar().showMessage("⏳ restic не найден — скачиваю…", 0)
        def _do():
            try:
                self._download_restic_to(managed)
                err = None
            except Exception as e:
                err = str(e)
            def _ui():
                self._restic_net_busy = False
                if err is None:
                    self.statusBar().showMessage("✅ restic загружен", 5000)
                else:
                    self.statusBar().showMessage(f"❌ Не удалось загрузить restic: {err}", 8000)
                    self._log(f"Auto-download restic failed: {err}", "ERROR", channel="error")
            self._ui_call_signal.emit(_ui)
        threading.Thread(target=_do, daemon=True).start()

    def _config_warn_ui(self, msg: str):
        """Хук Config → пользователь. Может звать фоновый поток, поэтому
        показываем окно строго через _ui_call_signal."""
        if getattr(self, "_closing", False):
            return
        # _save_global подряд дёргает десяток sg() — не вываливаем десять окон.
        if getattr(self, "_config_warn_busy", False):
            return
        self._config_warn_busy = True
        def _show():
            # singleShot(0) уже В UI-потоке: показываем окно ПОСЛЕ выхода из sg(),
            # чтобы модальный диалог не держал межпроцессный _ConfigLock.
            def _box():
                try:
                    QMessageBox.warning(self, "Настройки не сохранены", msg)
                finally:
                    self._config_warn_busy = False
            QTimer.singleShot(0, _box)
        self._ui_call_signal.emit(_show)

    def _run_bg_wait(self, fn, timeout_ms: int = 180000):
        """Выполняет блокирующую fn() в фоновом потоке, но НЕ морозит UI:
        локальный QEventLoop продолжает обрабатывать события Qt, пока поток
        работает. Нужен там, где вызывающий код синхронный и обязан вернуть
        результат (проверки службы агента / WebClient).

        Два инварианта, которые легко потерять:
        1) выход из цикла продублирован сторожевым QTimer — если emit не дошёл
           (C++-объект окна уже удалён, сигнал бросил), вложенный цикл иначе
           висел бы вечно и GUI не отвечал и не закрывался;
        2) ExcludeUserInputEvents — иначе кнопки продолжают принимать клики во
           время ожидания (до 6 с у агента, до 120 с у DISM), и пользователь
           запускает второй POST /mount/start, второй dism, второй sc start;
           флаги _rsvr_busy/_backup_running выставляются позже и не спасают."""
        box = {}
        loop = QEventLoop()
        def _quit():
            if loop.isRunning():
                loop.quit()
        def _worker():
            try:
                box["r"] = fn()
            except BaseException as e:   # noqa: BLE001 — пробрасываем наверх как есть
                box["e"] = e
            box["done"] = True           # ставим ДО emit: сторож увидит в любом случае
            try:
                self._ui_call_signal.emit(_quit)
            except BaseException:
                pass                     # окно уже уничтожено — выйдем по сторожу
        guard = QTimer()
        guard.setInterval(200)
        elapsed = {"ms": 0}
        def _tick():
            elapsed["ms"] += guard.interval()
            if box.get("done"):
                _quit()
            elif elapsed["ms"] >= timeout_ms:
                # "r" уже лежит в box → воркер успел отработать, просто не выставил
                # ещё box["done"]. Ставить таймаутную ошибку нельзя: успешный
                # результат превратился бы в RuntimeError.
                if "r" not in box:
                    box.setdefault("e", RuntimeError(
                        f"операция не завершилась за {timeout_ms // 1000} с"))
                _quit()
        guard.timeout.connect(_tick)
        guard.start()
        threading.Thread(target=_worker, daemon=True).start()
        # ExcludeUserInputEvents гасит клики молча — без песочных часов окно
        # читается как зависшее (проверки агента до 6 с, DISM до 120 с).
        QApplication.setOverrideCursor(Qt.WaitCursor)
        try:
            loop.exec_(QEventLoop.ExcludeUserInputEvents)
        finally:
            QApplication.restoreOverrideCursor()
            guard.stop()
        if "e" in box:
            raise box["e"]
        return box.get("r")

    @property
    def worker(self) -> Optional[ResticWorker]:
        return self._workers.get(self.pid)

    # ─── UI BUILD ───────────────────────────────────────────────────────────

    def _init_ui(self):
        self.setWindowTitle(f"RestLex v{APP_VERSION}")
        self.setAcceptDrops(True)   # перетащить экспорт (.json) на окно = импорт
        self.resize(1200,720)
        self.setMinimumSize(900, 560)
        ic = app_icon()
        if not ic.isNull():
            self.setWindowIcon(ic)
            QApplication.instance().setWindowIcon(ic)

        central=QWidget(); self.setCentralWidget(central)
        root=QHBoxLayout(central); root.setContentsMargins(6,0,6,6); root.setSpacing(6)

        # Sidebar
        side=QWidget(); side.setMaximumWidth(228); side.setMinimumWidth(190)
        sl=QVBoxLayout(side); sl.setContentsMargins(0,0,0,0); sl.setSpacing(4)
        lbl_p=QLabel(f"RestLex v{APP_VERSION}")
        lbl_p.setFixedHeight(26)
        lbl_p.setStyleSheet("background:#e0e8f8;font-weight:bold;border:1px solid #ccd7ee;padding-left:8px;")
        sl.addWidget(lbl_p)
        self.plist=QListWidget()
        self.plist.setItemDelegate(_ProfItemDelegate(self.plist))
        self.plist.itemClicked.connect(self._on_prof_click)
        self.plist.setContextMenuPolicy(Qt.CustomContextMenu)
        self.plist.customContextMenuRequested.connect(self._prof_ctx)
        sl.addWidget(self.plist,1)

        pbtn=QHBoxLayout()
        for lbl,fn in [("➕ Новый",self._add_prof)]:
            b=QPushButton(lbl); b.clicked.connect(fn); pbtn.addWidget(b)
        sl.addLayout(pbtn)

        pbtn2=QHBoxLayout()
        for lbl,fn in [("⬇ Импорт",self._import_prof),("⬆ Экспорт",self._export_prof)]:
            b=QPushButton(lbl); b.clicked.connect(fn); pbtn2.addWidget(b)
        sl.addLayout(pbtn2)
        pbtn3=QHBoxLayout()
        for lbl,fn in [("Импорт всего",self._import_all),("Экспорт всего",self._export_all)]:
            b=QPushButton(lbl); b.clicked.connect(fn); pbtn3.addWidget(b)
        sl.addLayout(pbtn3)

        # Rename profile inline on double click
        self.plist.itemDoubleClicked.connect(self._rename_prof)

        # Tabs
        self.tabs=QTabWidget()
        self.tabs.currentChanged.connect(self._on_tab)
        self._build_backup()
        self._build_restore()
        self._build_repo()
        self._build_retention()
        self._build_logs()
        self._build_scheduler()
        self._build_settings()
        self._build_web_tab()

        # Service status badge (corner widget on tab bar) — виден только при проблемах
        self.lbl_service_badge = QLabel("")
        self.lbl_service_badge.setOpenExternalLinks(False)
        self.lbl_service_badge.linkActivated.connect(self._open_service_settings)
        self.lbl_service_badge.setCursor(Qt.PointingHandCursor)
        self.lbl_service_badge.setVisible(False)
        self.tabs.setCornerWidget(self.lbl_service_badge, Qt.TopRightCorner)

        sp=QSplitter(Qt.Horizontal)
        sp.addWidget(side); sp.addWidget(self.tabs); sp.setStretchFactor(1,6)
        root.addWidget(sp,1)
        self._current_tab_widget = self.tabs.currentWidget()
        self.statusBar().showMessage("Готов")
        self.lbl_global_bk_banner = QLabel("")
        self.lbl_global_bk_banner.setStyleSheet(
            "color:#fff;background:#2e7d32;padding:2px 10px;border-radius:3px;font-weight:bold;"
        )
        self.lbl_global_bk_banner.setVisible(False)
        self.lbl_global_bk_banner.setCursor(Qt.PointingHandCursor)
        self.lbl_global_bk_banner.mousePressEvent = lambda _e: self._jump_to_running_backup()
        self.statusBar().addPermanentWidget(self.lbl_global_bk_banner)
        self._upd_title()
        # CLI auto-backup
        if getattr(self, "_cli_run_backup_pid", None):
            QTimer.singleShot(500, self._start_cli_backup_when_ready)

    def _start_cli_backup_when_ready(self, waited_ms: int = 0):
        """Гейт «restic готов». _ensure_restic_present докачивает бинарник в
        ФОНЕ (раньше конструктор блокировался до конца загрузки), поэтому
        `restlex.exe --run-backup <pid>` на машине без restic успевал стартовать
        до появления файла и падал. Ждём завершения загрузки, но не вечно."""
        if getattr(self, "_closing", False):
            return
        if getattr(self, "_restic_net_busy", False) and waited_ms < 300000:
            self.statusBar().showMessage("⏳ Жду загрузку restic перед авто-бэкапом…", 0)
            QTimer.singleShot(1000, lambda: self._start_cli_backup_when_ready(waited_ms + 1000))
            return
        self._run_cli_backup()

    def _normalize_restore_ui_mode(self, mode: str) -> str:
        value = str(mode or "").strip().lower()
        if value not in ("old", "new"):
            value = "old"
        if value == "new" and not _HAS_SHELL_EMBED:
            value = "old"
        return value

    def _apply_restore_ui_mode(self, save=False):
        mode = "new"
        self._restore_ui_mode = mode
        if save:
            self.cfg.restore_ui_mode = mode
        show_old = False
        if getattr(self, "restore_left_panel", None) is not None:
            self.restore_left_panel.setVisible(show_old)
        if getattr(self, "restore_splitter", None) is not None:
            self.restore_splitter.setHandleWidth(0)
            QTimer.singleShot(0, lambda: self.restore_splitter.setSizes([0, 1280]))
        browser_controls_visible = False
        for btn in (getattr(self, "btn_snap_browser", None), getattr(self, "btn_snap_tree", None)):
            if btn:
                btn.setVisible(browser_controls_visible)
        self._close_snap_browser()
        self._set_snap_view_mode("tree")

    def _on_restore_ui_mode_changed(self):
        self._restore_ui_mode = "new"
        self._apply_restore_ui_mode(save=True)

    def _update_restore_visibility(self, has_snapshots: bool, message: str = "", loading: bool = False):
        if not hasattr(self, "restore_content") or not hasattr(self, "restore_empty_lbl"):
            return
        show_content = bool(has_snapshots)
        self.restore_content.setVisible(show_content)
        self.restore_empty_box.setVisible(not show_content)
        self.restore_loading_bar.setVisible(loading and not show_content)
        if not show_content:
            if loading:
                text = "Загружаю список снапшотов…"
            else:
                text = message or "Бэкапов пока нет.\nСначала запустите бэкап для этого профиля."
            self.restore_empty_lbl.setText(text)
            self._close_snap_browser()

    # ─── BACKUP TAB ─────────────────────────────────────────────────────────

    def _build_backup(self):
        tab=QWidget(); ly=QVBoxLayout(tab); ly.setSpacing(8)
        self._tab_backup = tab

        # Sources
        sg=QGroupBox("Каталоги для резервного копирования"); sgl=QVBoxLayout(sg)
        self.src_list=QListWidget(); self.src_list.setMaximumHeight(180)
        self.src_list.itemDoubleClicked.connect(lambda _: self._edit_src())
        sgl.addWidget(self.src_list)
        sb=QHBoxLayout()
        for lbl,fn in [("📁 Добавить",self._add_src),("📄 Файлы",self._add_src_files),
                       ("✏️ Изменить",self._edit_src),
                       ("🗑 Удалить",self._del_src)]:
            b=QPushButton(lbl); b.clicked.connect(fn); sb.addWidget(b)
        btn_import=QPushButton("📄 Из списка"); btn_import.setFixedHeight(28)
        btn_import.setToolTip("Импортировать пути из текстового файла (один путь на строку)")
        btn_import.clicked.connect(self._import_sources_from_file)
        sb.addWidget(btn_import); sb.addStretch()
        sgl.addLayout(sb); ly.addWidget(sg)

        # Exclusions
        xg=QGroupBox("Исключения"); xgl=QVBoxLayout(xg)
        self.excl_list=QListWidget(); self.excl_list.setMaximumHeight(110)
        def _excl_dbl(item):
            v,ok=QInputDialog.getText(self,"Редактировать исключение","Маска:",text=item.text())
            if ok and v.strip(): item.setText(v.strip()); self._autosave(); self._saved()
        self.excl_list.itemDoubleClicked.connect(_excl_dbl)
        xgl.addWidget(self.excl_list)
        xb=QHBoxLayout()
        btn_xa=QPushButton("+ Паттерн"); btn_xa.setFixedWidth(100)
        btn_xf=QPushButton("📁 Папку"); btn_xf.setFixedWidth(90)
        btn_xd=QPushButton("🗑 Удалить"); btn_xd.setFixedWidth(100)
        btn_xp=QPushButton("Пресеты ▾"); btn_xp.setFixedWidth(110)
        def _excl_add():
            v,ok=QInputDialog.getText(self,"Добавить исключение","Маска (например *.tmp, node_modules):")
            if ok and v.strip(): self.excl_list.addItem(v.strip()); self._autosave(); self._saved()
        def _excl_del():
            r=self.excl_list.currentRow()
            if r>=0: self.excl_list.takeItem(r); self._autosave(); self._saved()
        def _excl_preset():
            m=QMenu(self)
            for name,items in [
                ("Windows temp",["*.tmp","*.log","Thumbs.db","desktop.ini"]),
                ("Dev (node_modules/.git)",["node_modules",".git","__pycache__","*.pyc","*.pyo"]),
                ("Логи",["*.log","*.bak","*.old"]),
            ]:
                act=m.addAction(name); act.setData(items)
            chosen=m.exec_(btn_xp.mapToGlobal(btn_xp.rect().bottomLeft()))
            if chosen:
                existing={self.excl_list.item(i).text() for i in range(self.excl_list.count())}
                for it in chosen.data():
                    if it not in existing: self.excl_list.addItem(it)
                self._autosave(); self._saved()
        def _excl_add_folder():
            p=QFileDialog.getExistingDirectory(self,"Выберите папку для исключения")
            if p:
                existing={self.excl_list.item(i).text() for i in range(self.excl_list.count())}
                if p not in existing: self.excl_list.addItem(p); self._autosave(); self._saved()
        btn_xa.clicked.connect(_excl_add); btn_xf.clicked.connect(_excl_add_folder)
        btn_xd.clicked.connect(_excl_del); btn_xp.clicked.connect(_excl_preset)
        xb.addWidget(btn_xa); xb.addWidget(btn_xf); xb.addWidget(btn_xd); xb.addWidget(btn_xp); xb.addStretch()
        xgl.addLayout(xb); ly.addWidget(xg)

        # Статус бэкапа без отдельного заголовка — он дублирует содержимое блока.
        pg=QFrame(); pg.setObjectName("bkStatusFrame")
        # По objectName: голый «QFrame {…}» цеплял и все QLabel внутри (QLabel — QFrame).
        pg.setStyleSheet("QFrame#bkStatusFrame { border:1px solid #cccccc; border-radius:4px; background:#ffffff; }")
        pgl=QVBoxLayout(pg)
        self.lbl_bk_status=QLabel("Готов к запуску")
        self.lbl_bk_status.setStyleSheet("font-weight:bold;font-size:12px;")
        self.lbl_bk_vss=QLabel("VSS = —")
        self.lbl_bk_vss.setStyleSheet("color:#555;font-size:10.5pt;")
        self.lbl_bk_vss.setVisible(False)
        self.lbl_bk_result=QLabel("Статус: —")
        self.lbl_bk_result.setTextFormat(Qt.RichText)
        self.lbl_bk_result.setTextInteractionFlags(Qt.TextBrowserInteraction)
        self.lbl_bk_result.setOpenExternalLinks(False)
        self.lbl_bk_result.linkActivated.connect(self._show_backup_error_logs_popup)
        self.lbl_bk_result.setStyleSheet("color:#333;font-size:10.5pt;")
        self.lbl_bk_file=QLabel("")
        self.lbl_bk_file.setWordWrap(True)
        self.lbl_bk_file.setStyleSheet("color:#555;font-size:10.5pt;")
        self.lbl_bk_stats=QLabel("")
        self.lbl_bk_stats.setStyleSheet("color:#336;font-size:10px;")
        self.prog_bk=QProgressBar(); self.prog_bk.setRange(0,0); self.prog_bk.setVisible(False)
        self.prog_bk.setFixedHeight(18)
        self.prog_bk.setStyleSheet(
            "QProgressBar{border:1px solid #aaa;border-radius:3px;background:#e8e8e8;text-align:center;font-size:9px;}"
            "QProgressBar::chunk{background:#3cb371;width:18px;margin:1px;border-radius:2px;}"
        )
        pgl.addWidget(self.prog_bk)
        pgl.addWidget(self.lbl_bk_vss)
        pgl.addWidget(self.lbl_bk_status)
        pgl.addWidget(self.lbl_bk_result)
        pgl.addWidget(self.lbl_bk_file)
        pgl.addWidget(self.lbl_bk_stats)
        self._set_bk_status_line("")
        self._set_bk_vss_status(None)
        self._set_bk_result_status("idle")
        ly.addWidget(pg)

        # Buttons
        bg=QGroupBox("Запуск"); bgl=QVBoxLayout(bg)
        self.btn_bk=QPushButton("🚀 Запустить бэкап"); self.btn_bk.setFixedHeight(36)
        self.btn_bk.clicked.connect(self._run_backup)
        self.btn_bk.setStyleSheet("font-weight:bold;font-size:12px;background:#ddeeff;border:1px solid #99bbdd;")
        self.btn_bk_pause=QPushButton("⏸ Пауза")
        self.btn_bk_pause.setFixedHeight(32)
        self.btn_bk_pause.setStyleSheet("font-weight:bold;color:#444;background:#fff8dd;border:1px solid #ddcc88;")
        self.btn_bk_pause.clicked.connect(self._toggle_pause_backup)
        self.btn_bk_cancel=QPushButton("✕ Отменить")
        self.btn_bk_cancel.setFixedHeight(32)
        self.btn_bk_cancel.setStyleSheet("font-weight:bold;color:#cc0000;background:#ffeeee;border:1px solid #cc8888;")
        self.btn_bk_cancel.clicked.connect(self._cancel_backup)
        self.bk_ctrl_row = QWidget()
        bk_ctrl_ly = QHBoxLayout(self.bk_ctrl_row); bk_ctrl_ly.setContentsMargins(0,0,0,0); bk_ctrl_ly.setSpacing(6)
        bk_ctrl_ly.addWidget(self.btn_bk_pause, 1)
        bk_ctrl_ly.addWidget(self.btn_bk_cancel, 1)
        self.bk_ctrl_row.setVisible(False)
        bgl.addWidget(self.btn_bk)
        bgl.addWidget(self.bk_ctrl_row)
        self.lbl_bk_note=QLabel("После бэкапа автоматически применяются правила хранения, если они заданы.")
        self.lbl_bk_note.setWordWrap(True)
        self.lbl_bk_note.setStyleSheet("color:#555;")
        bgl.addWidget(self.lbl_bk_note)
        ly.addWidget(bg)

        self.tabs.addTab(tab,"💾 Бэкап")

    # ─── RESTORE TAB ────────────────────────────────────────────────────────

    def _build_restore(self):
        tab=QWidget(); outer=QVBoxLayout(tab); outer.setContentsMargins(6,6,6,6); outer.setSpacing(4)
        self._tab_restore = tab
        # Снимки грузятся сами при открытии вкладки; пока идёт загрузка — бегущий прогресс-бар.
        self.restore_empty_box = QWidget()
        _eb = QVBoxLayout(self.restore_empty_box); _eb.setContentsMargins(0, 0, 0, 0); _eb.setSpacing(12)
        self.restore_empty_lbl=QLabel("Бэкапов пока нет.\nСначала запустите бэкап для этого профиля.")
        self.restore_empty_lbl.setAlignment(Qt.AlignCenter)
        self.restore_empty_lbl.setWordWrap(True)
        self.restore_empty_lbl.setStyleSheet("color:#666;font-size:14px;font-weight:600;")
        self.restore_loading_bar = QProgressBar()
        self.restore_loading_bar.setRange(0, 0)
        self.restore_loading_bar.setTextVisible(False)
        self.restore_loading_bar.setFixedSize(320, 14)
        self.restore_loading_bar.hide()
        _eb.addStretch(1)
        _eb.addWidget(self.restore_empty_lbl)
        _eb.addWidget(self.restore_loading_bar, 0, Qt.AlignHCenter)
        _eb.addStretch(1)
        outer.addWidget(self.restore_empty_box, 1)
        self.restore_content=QWidget()
        rl=QVBoxLayout(self.restore_content); rl.setContentsMargins(0,0,0,0); rl.setSpacing(4)
        self.restore_splitter = None
        self.restore_left_panel = None
        self.local_edit = QLineEdit()
        self.local_tree = DropTree()
        self.drives_row = QHBoxLayout()

        rw=QWidget(); rrly=QVBoxLayout(rw); rrly.setContentsMargins(0,0,0,0); rrly.setSpacing(4)
        self.snap_path_lbl=QLineEdit(rw)
        self.snap_path_lbl.setReadOnly(True)
        self.snap_path_lbl.hide()
        self.snap_load_lbl=QLabel(rw)
        self.snap_load_lbl.hide()

        snapbar=QHBoxLayout(); snapbar.setSpacing(4); snapbar.setContentsMargins(0,0,0,0)
        self.snap_combo=QComboBox()
        self.snap_combo.setSizeAdjustPolicy(QComboBox.AdjustToContents)
        self.snap_combo.setFixedHeight(28)
        self.snap_combo.setSizePolicy(QSizePolicy.Preferred, QSizePolicy.Fixed)
        self.snap_combo.currentIndexChanged.connect(self._on_snap_combo)
        self.snap_combo.setPlaceholderText("Снапшот…")
        self.snap_search_edit = QLineEdit()
        self.snap_search_edit.setPlaceholderText("Поиск файлов - файл1, файл2, ...")
        self.snap_search_edit.setFixedHeight(28)
        self.snap_search_edit.setMinimumWidth(120)
        self.snap_search_edit.textChanged.connect(self._on_snap_search_changed)
        self.btn_search_clear = QPushButton("✕"); self.btn_search_clear.setFixedSize(24, 28)
        self.btn_search_clear.setToolTip("Очистить поиск"); self.btn_search_clear.setVisible(False)
        self.btn_search_clear.clicked.connect(lambda: self.snap_search_edit.clear())
        self.lbl_search_status = QLabel(""); self.lbl_search_status.setStyleSheet("color:#555;font-size:9pt;")
        _del_btn_css = (
            "QPushButton{padding:0 12px;font-weight:600;color:#1a1a1a;background:#f3f3f3;"
            "border:1px solid #b5b5b5;border-radius:4px;}"
            "QPushButton:hover{background:#e8eef7;}"
            "QPushButton:checked{background:#fde8e6;border:1px solid #e0a39e;color:#8c1d18;}"
            "QPushButton:disabled{color:#999;background:#f3f3f3;border:1px solid #d5d5d5;}")
        _del_chk_css = (
            "QCheckBox{padding:0 12px 0 8px;font-weight:600;color:#1a1a1a;background:#f3f3f3;"
            "border:1px solid #b5b5b5;border-radius:4px;spacing:7px;}"
            "QCheckBox:hover{background:#e8eef7;}"
            "QCheckBox:checked{background:#fde8e6;border:1px solid #e0a39e;color:#8c1d18;}"
            "QCheckBox:disabled{color:#999;background:#f3f3f3;border:1px solid #d5d5d5;}")
        self.btn_show_deleted = QPushButton("Показать удалённые в выбранном каталоге")
        self.btn_show_deleted.setFixedHeight(28)
        self.btn_show_deleted.setStyleSheet(_del_btn_css)
        self.btn_show_deleted.setToolTip(
            "Показать файлы выбранной папки И ВСЕХ ПАПОК НИЖЕ, которых больше нет на диске.\n"
            "Сначала выделите папку в снимке слева; результат откроется справа.")
        self.btn_show_deleted.clicked.connect(self._on_show_deleted_clicked)
        self.chk_only_deleted = QCheckBox("Показывать только удалённые")
        self.chk_only_deleted.setFixedHeight(28)
        self.chk_only_deleted.setStyleSheet(_del_chk_css)
        self.chk_only_deleted.setToolTip(
            "Справа в реальном времени показывается то же дерево, но только удалённые файлы\n"
            "выбранной слева папки (и всех папок ниже). Выбрали другую папку — справа она обновится сама.")
        self.chk_only_deleted.toggled.connect(self._on_only_deleted_toggled)
        self.lbl_del_status = QLabel("")
        self.lbl_del_status.setStyleSheet("color:#333;font-weight:600;")
        self.del_progress = QProgressBar()      # небольшой бегущий бар справа от кнопок, пока идёт сравнение
        self.del_progress.setRange(0, 0)
        self.del_progress.setTextVisible(False)
        self.del_progress.setFixedSize(170, 12)
        self.del_progress.hide()
        self.btn_del_cancel = QPushButton("Отмена")
        self.btn_del_cancel.setFixedHeight(24)
        self.btn_del_cancel.setStyleSheet("QPushButton{padding:0 10px;font-weight:600;color:#8c1d18;}")
        self.btn_del_cancel.clicked.connect(self._on_del_cancel)
        self.btn_del_cancel.hide()
        self._auto_scan_timer = QTimer(self)    # «Показывать только удалённые»: пересчёт после выбора другой папки
        self._auto_scan_timer.setSingleShot(True)
        self._auto_scan_timer.setInterval(350)
        self._auto_scan_timer.timeout.connect(lambda: self._start_folder_scan(silent=True))
        self.btn_snap_del=QPushButton(); self.btn_snap_del.setFixedSize(28,28)
        self.btn_snap_del.setIcon(line_icon("trash", "#b3261e")); self.btn_snap_del.setIconSize(QSize(20,20))
        self.btn_snap_del.setToolTip("Удалить снапшот")
        self.btn_snap_del.clicked.connect(self._del_snap)
        self.btn_snap_ref=QPushButton(); self.btn_snap_ref.setFixedSize(28,28)
        self.btn_snap_ref.setIcon(line_icon("refresh", "#1f5fa8")); self.btn_snap_ref.setIconSize(QSize(20,20))
        self.btn_snap_ref.setToolTip("Обновить список снапшотов")
        self.btn_snap_ref.clicked.connect(self._refresh_snaps)
        snapbar.addWidget(self.snap_combo)
        snapbar.addWidget(self.snap_search_edit, 1)
        snapbar.addWidget(self.btn_search_clear)
        snapbar.addWidget(self.lbl_search_status)
        snapbar.addWidget(self.btn_snap_del)
        snapbar.addWidget(self.btn_snap_ref)
        rrly.addLayout(snapbar)
        delbar = QHBoxLayout(); delbar.setSpacing(6); delbar.setContentsMargins(0, 0, 0, 0)
        delbar.addWidget(self.btn_show_deleted)
        delbar.addWidget(self.chk_only_deleted)
        delbar.addWidget(self.del_progress)
        delbar.addWidget(self.lbl_del_status, 1)
        delbar.addWidget(self.btn_del_cancel)
        rrly.addLayout(delbar)

        self.snap_tree=DragTree()
        self.snap_tree.setHeaderLabels(["Имя","Размер","Дата"])
        self.snap_tree.header().setSectionResizeMode(0,QHeaderView.Stretch)
        self.snap_tree.header().resizeSection(1,90)
        self.snap_tree.header().resizeSection(2,130)
        self.snap_tree.setSelectionMode(QAbstractItemView.ExtendedSelection)
        self.snap_tree.setAlternatingRowColors(True)
        self.snap_tree.setRootIsDecorated(True)
        self.snap_tree.setItemsExpandable(True)
        self.snap_tree.setExpandsOnDoubleClick(True)
        self.snap_tree.setToolTip("Перетащите файлы влево для восстановления")
        self.snap_tree.itemSelectionChanged.connect(self._sync_snap_path_label)
        self.snap_tree.itemExpanded.connect(self._on_snap_item_expanded)
        self.snap_tree.setContextMenuPolicy(Qt.CustomContextMenu)
        self.snap_tree.customContextMenuRequested.connect(self._snap_ctx)
        self.snap_view_stack=QStackedWidget()
        self.snap_browser_host=None
        self.snap_browser_hint=None
        if _HAS_SHELL_EMBED:
            try:
                self.snap_browser_host=QFrame()
                self.snap_browser_host.setFrameShape(QFrame.StyledPanel)
                self.snap_browser_host.setStyleSheet("background:#ffffff;")
                self.snap_browser_host.setAttribute(Qt.WA_NativeWindow, True)
                bl=QVBoxLayout(self.snap_browser_host)
                bl.setContentsMargins(0,0,0,0)
                self.snap_browser_hint=QLabel("Explorer mount откроется здесь")
                self.snap_browser_hint.setAlignment(Qt.AlignCenter)
                self.snap_browser_hint.setStyleSheet("color:#666;font-style:italic;")
                bl.addWidget(self.snap_browser_hint)
                self.snap_view_stack.addWidget(self.snap_browser_host)
            except Exception:
                self.snap_browser_host=None
                self.snap_browser_hint=None
        self.snap_view_stack.addWidget(self.snap_tree)

        # Search results page (added as last page in stack)
        self.snap_search_list = QListWidget()
        self.snap_search_list.setAlternatingRowColors(True)
        self.snap_search_list.setSelectionMode(QAbstractItemView.ExtendedSelection)
        self.snap_search_list.setFont(QFont("Consolas", 9))
        self.snap_search_list.setToolTip("Двойной клик или кнопка — восстановить выбранное")
        self.snap_search_list.itemDoubleClicked.connect(lambda _: self._restore_selected_to_documents())
        self.snap_search_list.setContextMenuPolicy(Qt.CustomContextMenu)
        self.snap_search_list.customContextMenuRequested.connect(self._snap_search_ctx)
        self.snap_view_stack.addWidget(self.snap_search_list)

        # Правая часть: то же дерево, что слева, но только с удалённым. Пустое, пока не нажата кнопка/галка.
        self.snap_deleted = QTreeWidget()
        self.snap_deleted.setHeaderLabels(["Имя", "Размер", "Дата"])
        self.snap_deleted.header().setSectionResizeMode(0, QHeaderView.Stretch)
        self.snap_deleted.header().resizeSection(1, 90)
        self.snap_deleted.header().resizeSection(2, 130)
        self.snap_deleted.setSelectionMode(QAbstractItemView.ExtendedSelection)
        self.snap_deleted.setAlternatingRowColors(True)
        self.snap_deleted.setRootIsDecorated(True)
        self.snap_deleted.setItemsExpandable(True)
        self.snap_deleted.setExpandsOnDoubleClick(True)
        self.snap_deleted.setToolTip("Удалённые файлы выбранной слева папки. Двойной клик — восстановить")
        self.snap_deleted.itemDoubleClicked.connect(
            lambda it, _c: self._restore_selected_to_documents() if it.data(0, Qt.UserRole + 6) == "deleted" else None)
        self.snap_deleted.setContextMenuPolicy(Qt.CustomContextMenu)
        self.snap_deleted.customContextMenuRequested.connect(self._snap_deleted_ctx)
        self.snap_deleted.itemSelectionChanged.connect(lambda: setattr(self, "_last_pick", "deleted"))
        self.snap_tree.itemSelectionChanged.connect(lambda: setattr(self, "_last_pick", "tree"))
        self.snap_tree.currentItemChanged.connect(self._on_snap_current_changed)
        self.snap_view_stack.setMinimumWidth(200)   # иначе страница «Проводник» не даёт сузить левую часть
        self.snap_deleted.setMinimumWidth(200)
        self.snap_split = EqualSplitter(Qt.Horizontal)
        self.snap_split.setChildrenCollapsible(False)
        self.snap_split.addWidget(self.snap_view_stack)
        self.snap_split.addWidget(self.snap_deleted)
        self.snap_split.setStretchFactor(0, 1)
        self.snap_split.setStretchFactor(1, 1)
        rrly.addWidget(self.snap_split, 1)
        self.snap_overlay = BusyPanel(self.snap_split)
        rrly.addWidget(self.snap_overlay, 1)

        rb=QHBoxLayout(); rb.setSpacing(4)
        self.btn_restore_inplace=QPushButton("Восстановить по месту хранения")
        self.btn_restore_inplace.setFixedHeight(34)
        self.btn_restore_inplace.setStyleSheet("font-weight:bold;background:#eef4ff;border:1px solid #b9cbe8;")
        self.btn_restore_inplace.clicked.connect(self._restore_selected_in_place)
        self.btn_restore_docs=QPushButton("Восстановить выбранное")
        self.btn_restore_docs.setFixedHeight(34)
        self.btn_restore_docs.setStyleSheet("font-weight:bold;background:#ddeeff;border:1px solid #99bbdd;")
        self.btn_restore_docs.clicked.connect(self._restore_selected_to_documents)
        self.btn_snap_open=QPushButton("Открыть в проводнике")
        self.btn_snap_open.setFixedHeight(34)
        self.btn_snap_open.setStyleSheet("font-weight:bold;background:#f4f4f4;border:1px solid #c8c8c8;")
        self.btn_snap_open.clicked.connect(self._open_selected_snapshot_in_explorer)
        rb.addWidget(self.btn_restore_inplace,1)
        rb.addWidget(self.btn_restore_docs,1)
        rb.addWidget(self.btn_snap_open,1)
        rrly.addLayout(rb)
        self._set_snap_view_mode("tree")
        if self.snap_browser_host:
            self._snap_browser_timer = QTimer(self)
            self._snap_browser_timer.setInterval(700)
            self._snap_browser_timer.timeout.connect(self._snap_browser_tick)
            self._snap_browser_timer.start()
        rl.addWidget(rw,1)

        self._apply_restore_ui_mode(save=False)
        outer.addWidget(self.restore_content,1)
        self._update_restore_visibility(False, message="Бэкапов пока нет.\nСначала запустите бэкап для этого профиля.")

        self.tabs.addTab(tab,"📥 Восстановление")
        # Индикатор «на вкладке идёт работа»: значок вкладки крутится, пока что-то грузится/сравнивается.
        self._rtab_frames = "🕐🕑🕒🕓🕔🕕🕖🕗🕘🕙🕚🕛"
        self._rtab_i = 0
        self._rtab_busy = False
        self._rtab_timer = QTimer(self)
        self._rtab_timer.setInterval(150)
        self._rtab_timer.timeout.connect(self._tick_restore_tab_busy)
        self._rtab_timer.start()

    def _tick_restore_tab_busy(self):
        idx = self.tabs.indexOf(self._tab_restore)
        if idx < 0:
            return
        busy = bool(self._del_mode or self._snap_loading or self._ls_worker or self._restore_in_progress)
        if busy:
            self._rtab_i = (self._rtab_i + 1) % len(self._rtab_frames)
            self.tabs.setTabText(idx, f"{self._rtab_frames[self._rtab_i]} Восстановление")
            self.tabs.tabBar().setTabTextColor(idx, QColor("#d9660a"))
            self._rtab_busy = True
        elif self._rtab_busy:
            self.tabs.setTabText(idx, "📥 Восстановление")
            self.tabs.tabBar().setTabTextColor(idx, QColor())
            self._rtab_busy = False

    # ─── REPO TAB ────────────────────────────────────────────────────────────

    def _build_repo(self):
        tab=QWidget(); ml=QVBoxLayout(tab)
        self._tab_repo = tab

        tg=QGroupBox("Тип репозитория"); tgl=QHBoxLayout(tg)
        self.rtype_list=QListWidget(); self.rtype_list.setFixedWidth(210)
        for _,lbl,*__ in REPO_TYPES: self.rtype_list.addItem(lbl)
        # фиксированная высота: иначе при раскрытии WOL лэйаут сжимал список (всё «ехало вверх»)
        # вместо того чтобы расти вниз со скроллом
        self.rtype_list.setFixedHeight(self.rtype_list.sizeHintForRow(0) * self.rtype_list.count()
                                       + 2 * self.rtype_list.frameWidth() + 4)
        self.rtype_list.currentRowChanged.connect(self._on_rtype)
        self.rtype_list.currentRowChanged.connect(lambda _=None: self._repo_changed())

        self.rstack=QStackedWidget(); self._rpages: Dict[str,dict]={}
        for rt,_,browse,hint,ex,_ in REPO_TYPES:
            pg,refs=self._make_rpage(rt,browse,hint,ex)
            self.rstack.addWidget(pg); self._rpages[rt]=refs

        tgl.addWidget(self.rtype_list); tgl.addWidget(self.rstack,1); ml.addWidget(tg)

        # Password + Save button
        pg2=QGroupBox("Пароль репозитория"); pg2l=QVBoxLayout(pg2)
        pg2l.addWidget(QLabel("Введите пароль или выберите файл (файл прочитается и пароль подставится):"))
        pr=QHBoxLayout()
        self.edit_pw=QLineEdit(); self.edit_pw.setEchoMode(QLineEdit.Password)
        self.edit_pw.setPlaceholderText("Пароль репозитория…")
        self.edit_pw.textEdited.connect(self._repo_changed)
        btn_eye=QToolButton(); btn_eye.setText("👁"); btn_eye.setCheckable(True); btn_eye.setFixedSize(28,26)
        btn_eye.toggled.connect(lambda on:self.edit_pw.setEchoMode(
            QLineEdit.Normal if on else QLineEdit.Password))
        self._pw_eye_btn = btn_eye
        btn_pf=QPushButton("📂 Из файла"); btn_pf.clicked.connect(self._load_pw_file)
        pr.addWidget(self.edit_pw,1); pr.addWidget(btn_eye); pr.addWidget(btn_pf)
        pg2l.addLayout(pr)
        btn_save_check_row=QHBoxLayout()
        self.btn_repo_save=QPushButton("💾 Сохранить"); self.btn_repo_save.setVisible(False)
        self.btn_repo_save.clicked.connect(self._save_repo); self.btn_repo_save.setEnabled(False)
        self.btn_chk=QPushButton("💾 Сохранить и проверить"); self.btn_chk.setFixedHeight(28)
        self.btn_chk.clicked.connect(self._save_check)
        self.lbl_chk=QLabel(""); self.lbl_chk.setWordWrap(True)
        btn_save_check_row.addWidget(self.btn_chk)
        btn_save_check_row.addWidget(self.lbl_chk,1)
        pg2l.addLayout(btn_save_check_row)
        ml.addWidget(pg2)

        # WOL и выключение — одна сетка: подписи | поле | кнопка, колонки одинаковые в обоих блоках
        MAC_PH = "MAC в любом виде: AA:BB:CC:DD:EE:FF, AA-BB-CC-DD-EE-FF, AA BB CC DD EE FF, AABBCCDDEEFF"
        LBL_W, BTN_W, SPIN_W, MAC_W = 145, 130, 100, 150

        def _grid(parent):
            g = QGridLayout(parent); g.setContentsMargins(0, 0, 0, 0)
            g.setHorizontalSpacing(8); g.setVerticalSpacing(6)
            g.setColumnMinimumWidth(0, LBL_W); g.setColumnStretch(1, 1); g.setColumnMinimumWidth(2, BTN_W)
            return g

        def _btn(text, tip, slot):
            b = QPushButton(text); b.setToolTip(tip); b.setFixedWidth(BTN_W); b.clicked.connect(slot)
            return b

        def _spin(lo, hi, val):
            sp = QSpinBox(); sp.setRange(lo, hi); sp.setValue(val); sp.setSuffix(" сек")
            sp.setFixedWidth(SPIN_W); sp.valueChanged.connect(self._save_power)
            return sp

        L = Qt.AlignLeft | Qt.AlignVCenter

        wg=QGroupBox("Wake-on-LAN перед бэкапом"); wgl=QVBoxLayout(wg)
        self.chk_wol=QCheckBox("Включить Wake-on-LAN")
        self.chk_wol.toggled.connect(self._save_power)
        wgl.addWidget(self.chk_wol)
        self.chk_wol.setToolTip("При запуске бэкапа отправляется Magic Packet по MAC-адресу. Если включено ожидание — "
                                "хост пингуется с заданным интервалом до истечения таймаута; не ответил → бэкап прерывается.")
        wol_sub=QWidget(); wg_=_grid(wol_sub)
        self._wol_sub=wol_sub
        self.chk_wol.toggled.connect(wol_sub.setVisible)
        wol_sub.setVisible(False)

        def _row(*items):
            h = QHBoxLayout(); h.setContentsMargins(0, 0, 0, 0); h.setSpacing(8)
            for it in items:
                h.addWidget(it) if isinstance(it, QWidget) else h.addSpacing(it)
            h.addStretch()
            return h

        self.edit_wol_mac=QLineEdit(); self.edit_wol_mac.setPlaceholderText("AA:BB:CC:DD:EE:FF")
        self.edit_wol_mac.setToolTip(MAC_PH); self.edit_wol_mac.setFixedWidth(MAC_W)
        self.edit_wol_mac.editingFinished.connect(self._save_power)
        self.edit_wol_host=QLineEdit(); self.edit_wol_host.setPlaceholderText("IP или имя ПК")
        self.edit_wol_host.setToolTip("IP или имя ПК. Нужен для ожидания пинга и для кнопки «MAC по IP»")
        self.edit_wol_host.setMinimumWidth(80); self.edit_wol_host.setMaximumWidth(180)
        self.edit_wol_host.editingFinished.connect(self._save_power)
        btn_mac_by_ip=QPushButton("MAC по IP")
        btn_mac_by_ip.setToolTip("Пингует хост (ПК должен быть включён) и подставляет его MAC из ARP-таблицы")
        btn_mac_by_ip.clicked.connect(self._wol_mac_by_ip)
        wg_.addWidget(QLabel("MAC / IP хоста:"), 0, 0)
        wg_.addLayout(_row(self.edit_wol_mac, self.edit_wol_host, btn_mac_by_ip), 0, 1)
        wg_.addWidget(_btn("Проверить", "Сохранить, разбудить ПК и открыть окно ping -t", self._wol_test), 0, 2)
        self.spin_wol_interval=_spin(1, 60, WOL_PING_INTERVAL_DEF)
        self.chk_wol_wait=QCheckBox("Ждать хост, не дольше:")
        self.chk_wol_wait.setToolTip("Ждать поднятия хоста перед бэкапом; не ответил за это время → бэкап прерывается")
        self.chk_wol_wait.setChecked(True)
        self.chk_wol_wait.toggled.connect(self._save_power)
        self.spin_wol_timeout=_spin(5, 3600, WOL_WAIT_TIMEOUT_DEF)
        self.chk_wol_wait.toggled.connect(self.spin_wol_timeout.setEnabled)
        wg_.addWidget(QLabel("Интервал пинга:"), 1, 0)
        wg_.addLayout(_row(self.spin_wol_interval, 8, self.chk_wol_wait, self.spin_wol_timeout), 1, 1)
        self.lbl_wol_status=QLabel(""); self.lbl_wol_status.setWordWrap(True); self.lbl_wol_status.setVisible(False)
        wg_.addWidget(self.lbl_wol_status, 2, 0, 1, 3)
        wgl.addWidget(wol_sub)
        ml.addWidget(wg)

        sg=QGroupBox("Выключение ПК после бэкапа"); sgl=QVBoxLayout(sg)
        sd_w=QWidget(); sg_=_grid(sd_w)
        self.chk_shutdown_after=QCheckBox("Выключить ЭТОТ ПК")
        self.chk_shutdown_after.toggled.connect(self._save_power)
        sg_.addWidget(self.chk_shutdown_after, 1, 0)
        self.spin_shutdown_delay=_spin(0, 3600, SHUTDOWN_DELAY_DEF)
        self.spin_shutdown_delay.setToolTip("Таймер shutdown только для ЭТОГО ПК")
        sg_.addWidget(self.spin_shutdown_delay, 1, 1, L)
        sg_.addWidget(_btn("Отмена выкл.", "shutdown /a — отменить запланированное выключение этого ПК", self._shutdown_abort), 1, 2)
        self.chk_shutdown_remote=QCheckBox("Выключить ПК по MAC:")
        self.chk_shutdown_remote.toggled.connect(self._save_power)
        self.edit_shutdown_mac=QLineEdit(); self.edit_shutdown_mac.setPlaceholderText("AA:BB:CC:DD:EE:FF")
        self.edit_shutdown_mac.setFixedWidth(MAC_W)  # одна ширина с MAC WOL
        self.edit_shutdown_mac.setToolTip(MAC_PH + ".\nОбычный MAC того ПК, как в getmac/ipconfig /all. Для пакета "
                                          "выключения программа перевернёт его сама — вручную ничего не переворачивай.")
        self.edit_shutdown_mac.editingFinished.connect(self._save_power)
        sg_.addWidget(self.chk_shutdown_remote, 2, 0)
        sg_.addWidget(self.edit_shutdown_mac, 2, 1, L)
        sg_.addWidget(_btn("Проверить", "Сохранить, отправить пакет выключения (Sleep-on-LAN) и открыть окно ping -t", self._shutdown_remote_test), 2, 2)
        self.lbl_sd_status=QLabel(""); self.lbl_sd_status.setWordWrap(True); self.lbl_sd_status.setVisible(False)
        sg_.addWidget(self.lbl_sd_status, 4, 0, 1, 3)
        sgl.addWidget(sd_w)
        ml.addWidget(sg)
        ml.addStretch()

        self.rtype_list.blockSignals(True)
        self.rtype_list.setCurrentRow(0)
        self.rtype_list.blockSignals(False)
        self.rstack.setCurrentIndex(0)
        self._mark_repo_saved()
        self._sync_encrypt_ui()
        # скролл вместо сжатия: в маленьком окне блоки WOL/выключения налезали друг на друга
        sc=QScrollArea(); sc.setWidgetResizable(True); sc.setFrameShape(QFrame.NoFrame); sc.setWidget(tab)
        # раскрыли WOL — докрутить, чтобы открывшаяся панель была видна целиком
        self.chk_wol.toggled.connect(lambda on: on and not getattr(self, "_power_loading", False)
                                    and QTimer.singleShot(0, lambda: sc.ensureWidgetVisible(wg, 0, 0)))
        self._tab_repo = sc
        self.tabs.addTab(sc,"🗄 Репозиторий")

    def _make_rpage(self,rt,browse,hint,ex):
        pg=QWidget(); ly=QVBoxLayout(pg); refs={}
        lh=QLabel(hint); lh.setWordWrap(True)
        lh.setStyleSheet("color:#555;font-size:10px;padding:2px 4px;background:#f8f8f8;border-radius:3px;")
        ly.addWidget(lh)

        if rt=="rest":
            for key,lbl,ph,isp in [
                ("rest_host","Хост:порт","192.168.1.10:8001",False),
                ("rest_path","Путь","/lex/",False),
                ("rest_user","Логин","lex",False),
                ("rest_pass","Пароль","●●●●●●",True),
            ]:
                row=QHBoxLayout(); l=QLabel(lbl); l.setMinimumWidth(80)
                ed=QLineEdit(); ed.setPlaceholderText(ph)
                if isp: ed.setEchoMode(QLineEdit.Password)
                ed.textEdited.connect(lambda _:(self._upd_rest_url(),self._repo_changed()))
                refs[key]=ed; row.addWidget(l); row.addWidget(ed,1); ly.addLayout(row)
            # URL display (editable — typing here auto-fills fields above)
            ur=QHBoxLayout(); ur.addWidget(QLabel("→ URL:"))
            refs["rest_url"]=QLineEdit()
            refs["rest_url"].setEchoMode(QLineEdit.Password)
            refs["rest_url"].setStyleSheet("font-family:Consolas;")
            refs["rest_url"].setPlaceholderText("rest:http://user:pass@host:8000/repo")
            # ponytail: схема (http/https) живёт в скрытом поле — попадает в сохранение как обычный ключ
            refs["rest_scheme"]=QLineEdit("http"); refs["rest_scheme"].hide()
            refs["rest_url"].textEdited.connect(lambda _,r=refs:(self._parse_rest_url(r),self._repo_changed()))
            be=QToolButton(); be.setText("👁"); be.setCheckable(True); be.setFixedSize(28,24)
            be.toggled.connect(lambda on,r=refs:r["rest_url"].setEchoMode(
                QLineEdit.Normal if on else QLineEdit.Password))
            ur.addWidget(refs["rest_url"],1); ur.addWidget(be); ly.addLayout(ur)
        else:
            ar=QHBoxLayout(); ar.addWidget(QLabel("Адрес:"))
            refs["addr"]=QLineEdit(); refs["addr"].setPlaceholderText(ex)
            refs["addr"].setFont(QFont("Consolas",9))
            refs["addr"].textEdited.connect(lambda _:(self._val_addr(),self._repo_changed()))
            ar.addWidget(refs["addr"],1)
            if browse:
                bb=QPushButton("📂"); bb.setFixedWidth(30)
                bb.clicked.connect(lambda _,r=refs:self._set_repo_addr_from_dialog(r))
                ar.addWidget(bb)
            ly.addLayout(ar)

            for key,lbl,ph,isp in self._creds(rt):
                cr=QHBoxLayout(); l=QLabel(lbl); l.setMinimumWidth(150)
                ed=QLineEdit(); ed.setPlaceholderText(ph)
                if isp: ed.setEchoMode(QLineEdit.Password)
                ed.textEdited.connect(self._repo_changed); refs[key]=ed
                cr.addWidget(l); cr.addWidget(ed,1); ly.addLayout(cr)

        refs["val"]=QLabel(""); refs["val"].setStyleSheet("color:#cc0000;")
        ly.addWidget(refs["val"]); ly.addStretch()
        return pg,refs

    def _set_repo_addr_from_dialog(self, refs: dict):
        current = refs.get("addr", QLineEdit()).text()
        path = QFileDialog.getExistingDirectory(self, "Папка") or current
        refs.get("addr", QLineEdit()).setText(path.replace("/", "\\"))
        self._val_addr()
        self._repo_changed()

    def _build_vss_group(self) -> QGroupBox:
        vg = QGroupBox("VSS")
        vgl = QVBoxLayout(vg)
        self.chk_vss = QCheckBox("Использовать VSS на Windows")
        self.chk_vss.setChecked(True)
        self.chk_vss.toggled.connect(self._changed)
        lbl_vss_note = QLabel("VSS создаёт теневой снимок диска перед бэкапом, чтобы корректно копировать открытые и занятые файлы.")
        lbl_vss_note.setWordWrap(True)
        lbl_vss_note.setStyleSheet("color:#555;")
        vgl.addWidget(self.chk_vss)
        vgl.addWidget(lbl_vss_note)
        return vg

    def _build_global_settings_group(self) -> QGroupBox:
        gg = QGroupBox("restic и TEMP")
        ggl = QVBoxLayout(gg)
        r1 = QHBoxLayout(); r1.addWidget(QLabel("restic.exe:"))
        self.edit_restic = QLineEdit(self.cfg.gg("restic", ""))
        self.edit_restic.setPlaceholderText(f"Пусто = {self.cfg.managed_restic_path()}")
        self.edit_restic.textEdited.connect(self._changed)
        self.edit_restic.textChanged.connect(lambda _: self._refresh_restic_status_label())
        b1 = QPushButton("📂"); b1.setFixedWidth(30)
        b1.clicked.connect(lambda: (self.edit_restic.setText(
            QFileDialog.getOpenFileName(self, "restic.exe", filter="*.exe")[0] or self.edit_restic.text()), self._changed()))
        r1.addWidget(self.edit_restic, 1); r1.addWidget(b1); ggl.addLayout(r1)
        rr = QHBoxLayout()
        self.btn_restic_check = QPushButton("🔎 Проверить версию")
        self.btn_restic_check.clicked.connect(self._check_restic_version)
        self.btn_restic_download = QPushButton("⬇ Загрузить новую версию")
        self.btn_restic_download.clicked.connect(self._download_restic_release)
        rr.addWidget(self.btn_restic_check)
        rr.addWidget(self.btn_restic_download)
        rr.addStretch()
        ggl.addLayout(rr)
        self.lbl_restic_status = QLabel("")
        self.lbl_restic_status.setWordWrap(True)
        self.lbl_restic_status.setStyleSheet("color:#555;")
        ggl.addWidget(self.lbl_restic_status)
        r2 = QHBoxLayout(); r2.addWidget(QLabel("TEMP:"))
        self.edit_temp = QLineEdit(self.cfg.temp); self.edit_temp.setPlaceholderText("Пусто = системный TEMP")
        self.edit_temp.textEdited.connect(self._changed)
        b2 = QPushButton("📂"); b2.setFixedWidth(30)
        b2.clicked.connect(lambda: (self.edit_temp.setText(
            QFileDialog.getExistingDirectory(self, "TEMP") or self.edit_temp.text()), self._changed()))
        r2.addWidget(self.edit_temp, 1); r2.addWidget(b2); ggl.addLayout(r2)
        restic_note = QLabel(
            f"Если путь пустой, RestLex будет использовать {self.cfg.managed_restic_path()}. "
            "Кнопка загрузки скачивает туда последнюю версию restic."
        )
        restic_note.setWordWrap(True)
        restic_note.setStyleSheet("color:#555;")
        ggl.addWidget(restic_note)
        return gg

    def _build_mount_group(self) -> QGroupBox:
        mg = QGroupBox("Mount в Explorer (WebDAV)")
        mgl = QVBoxLayout(mg)
        lbl_mount_note = QLabel(
            "Read-only mount через локальную службу RestlexAgent. "
            "WinFsp не нужен: Explorer открывает путь вида \\\\ИМЯ-ПК@порт\\DavWWWRoot."
        )
        lbl_mount_note.setWordWrap(True)
        lbl_mount_note.setStyleSheet("color:#555;")
        mgl.addWidget(lbl_mount_note)
        mr = QHBoxLayout()
        self.btn_mount_start = QPushButton("📂 Подключить")
        self.btn_mount_start.clicked.connect(self._mount_start)
        self.btn_mount_open = QPushButton("🗁 Открыть")
        self.btn_mount_open.clicked.connect(self._mount_open)
        self.btn_mount_open.setEnabled(False)
        self.btn_mount_stop = QPushButton("⏏ Отключить")
        self.btn_mount_stop.clicked.connect(self._mount_stop)
        self.btn_mount_stop.setEnabled(False)
        self.btn_mount_ref = QPushButton("↻")
        self.btn_mount_ref.setFixedWidth(28)
        self.btn_mount_ref.setToolTip("Обновить статус mount")
        self.btn_mount_ref.clicked.connect(self._mount_refresh_status)
        mr.addWidget(self.btn_mount_start)
        mr.addWidget(self.btn_mount_open)
        mr.addWidget(self.btn_mount_stop)
        mr.addWidget(self.btn_mount_ref)
        mr.addStretch()
        mgl.addLayout(mr)
        self.lbl_mount = QLabel("● mount не подключён")
        self.lbl_mount.setStyleSheet("color:gray;font-weight:bold;")
        mgl.addWidget(self.lbl_mount)
        self.ed_mount_path = QLineEdit()
        self.ed_mount_path.setReadOnly(True)
        self.ed_mount_path.setPlaceholderText(r"\\ИМЯ-ПК@порт\DavWWWRoot")
        mgl.addWidget(self.ed_mount_path)
        return mg

    def _build_backup_params_group(self) -> QGroupBox:
        bg = QGroupBox("Параметры бэкапа")
        bgl = QVBoxLayout(bg)

        self.chk_global_no_scan = QCheckBox("Без скана (--no-scan)")
        self.chk_global_no_scan.setToolTip("Не сканировать целевой каталог перед бэкапом. Ускоряет запуск, но прогресс-бар будет менее точным.")
        self.chk_global_no_scan.setChecked(self.cfg.global_no_scan)
        self.chk_global_no_scan.toggled.connect(self._changed)
        bgl.addWidget(self.chk_global_no_scan)

        self.chk_global_post_check = QCheckBox("Check после бэкапа")
        self.chk_global_post_check.setToolTip("После успешного бэкапа автоматически выполнять restic check для проверки целостности репозитория.")
        self.chk_global_post_check.setChecked(self.cfg.global_post_backup_check)
        self.chk_global_post_check.toggled.connect(self._changed)
        bgl.addWidget(self.chk_global_post_check)

        self.chk_global_pre_cmd = QCheckBox("CMD перед бэкапом")
        self.chk_global_pre_cmd.setToolTip("Команды, которые выполняются перед стартом бэкапа. Каждая строка — отдельная команда (shell). При ошибке любой из команд бэкап отменяется.")
        self.chk_global_pre_cmd.setChecked(bool(self.cfg.global_pre_backup_cmd.strip()))
        self.chk_global_pre_cmd.toggled.connect(self._changed)
        bgl.addWidget(self.chk_global_pre_cmd)

        self.edit_global_pre_cmd = QTextEdit()
        self.edit_global_pre_cmd.setPlainText(self.cfg.global_pre_backup_cmd)
        self.edit_global_pre_cmd.setPlaceholderText("Одна команда на строку")
        self.edit_global_pre_cmd.setMaximumHeight(80)
        self.edit_global_pre_cmd.textChanged.connect(self._changed)
        bgl.addWidget(self.edit_global_pre_cmd)

        row4 = QHBoxLayout()
        row4.addWidget(QLabel("Таймаут Telegram (часов):"))
        row4.addStretch()
        self.spin_global_bk_timeout = QSpinBox()
        self.spin_global_bk_timeout.setRange(0, 24)
        self.spin_global_bk_timeout.setValue(self.cfg.global_backup_timeout_hours)
        self.spin_global_bk_timeout.setSuffix(" ч")
        self.spin_global_bk_timeout.setToolTip("Если бэкап длится дольше указанного количества часов — отправляется уведомление в Telegram. 0 = не отслеживать.")
        self.spin_global_bk_timeout.valueChanged.connect(self._changed)
        row4.addWidget(self.spin_global_bk_timeout)
        bgl.addLayout(row4)

        return bg

    def _populate_integrations_layout(self, ly: QVBoxLayout):
        note = QLabel("⚠️ Глобальные настройки и службы")
        note.setStyleSheet("color:#885500;font-style:italic;")
        ly.addWidget(note)

        tg = QGroupBox("Telegram"); tgl = QVBoxLayout(tg)
        r1 = QHBoxLayout(); r1.addWidget(QLabel("API Token:"))
        self.ed_tgt = QLineEdit(self.cfg.tg_token); self.ed_tgt.setEchoMode(QLineEdit.Password)
        self.ed_tgt.textEdited.connect(self._changed); self.ed_tgt.editingFinished.connect(self._save_tg)
        r1.addWidget(self.ed_tgt,1); tgl.addLayout(r1)
        r2 = QHBoxLayout(); r2.addWidget(QLabel("ID чата:"))
        self.ed_tgc = QLineEdit(self.cfg.tg_chat); self.ed_tgc.setPlaceholderText("-100XXXXXXXXXX"); self.ed_tgc.textEdited.connect(self._changed); self.ed_tgc.editingFinished.connect(self._save_tg)
        r2.addWidget(self.ed_tgc,1); tgl.addLayout(r2)
        r2g = QHBoxLayout(); r2g.addWidget(QLabel("Тема (topic ID):"))
        self.ed_tg_group = QLineEdit(self.cfg.tg_group); self.ed_tg_group.setPlaceholderText("793 (опционально)"); self.ed_tg_group.textEdited.connect(self._changed); self.ed_tg_group.editingFinished.connect(self._save_tg)
        r2g.addWidget(self.ed_tg_group,1); tgl.addLayout(r2g)
        rb = QHBoxLayout()
        bt = QPushButton("📡 Проверить"); bt.clicked.connect(self._test_tg)
        self.lbl_tgs = QLabel("")
        rb.addWidget(bt); rb.addWidget(self.lbl_tgs); rb.addStretch()
        tgl.addLayout(rb); ly.addWidget(tg)

        rg = QGroupBox("Веб-отчёты и обновление службы"); rgl = QVBoxLayout(rg)
        rr1 = QHBoxLayout(); rr1.addWidget(QLabel("Report URL:"))
        self.ed_report_url = QLineEdit(self.cfg.report_url)
        self.ed_report_url.setPlaceholderText("https://example.com/api/restlex/report")
        self.ed_report_url.textEdited.connect(self._changed); self.ed_report_url.editingFinished.connect(self._save_tg)
        rr1.addWidget(self.ed_report_url,1); rgl.addLayout(rr1)
        rr2 = QHBoxLayout(); rr2.addWidget(QLabel("Update URL:"))
        self.ed_agent_update_url = QLineEdit(self.cfg.agent_auto_update_url)
        self.ed_agent_update_url.setPlaceholderText("https://example.com/restlex-agent.exe")
        self.ed_agent_update_url.textEdited.connect(self._changed); self.ed_agent_update_url.editingFinished.connect(self._save_tg)
        rr2.addWidget(self.ed_agent_update_url,1); rgl.addLayout(rr2)
        rdesc = QLabel(
            "Report URL получает JSON со статусом backup. "
            "Update URL пока только сохраняется в конфиге как задел под Rust-службу."
        )
        rdesc.setWordWrap(True); rdesc.setStyleSheet("color:#555;")
        rgl.addWidget(rdesc); ly.addWidget(rg)

        ag = QGroupBox("Служба RestlexAgent"); agl = QVBoxLayout(ag)
        arow = QHBoxLayout()
        self.lbl_agent_status = QLabel("—")
        self.lbl_agent_status.setStyleSheet("color:gray;font-weight:bold;")
        arow.addWidget(QLabel("Статус:"))
        arow.addWidget(self.lbl_agent_status)
        arow.addStretch()
        agl.addLayout(arow)
        abtn = QHBoxLayout()
        for lbl,fn,w in [
            ("🔄 Обновить агент", self._agent_sync_local, 150),
            ("➕ Установить", self._agent_install, 120),
            ("▶ Запустить", self._agent_start, 110),
            ("⏹ Остановить", self._agent_stop, 110),
            ("🗑 Удалить", self._agent_uninstall, 110),
        ]:
            b=QPushButton(lbl); b.setFixedWidth(w); b.clicked.connect(fn); abtn.addWidget(b)
        abtn.addStretch()
        agl.addLayout(abtn)
        adesc = QLabel(
            "Агент берётся из Rust-проекта и копируется рядом с Python-веткой. "
            "Он нужен для службы, веб-отчётов и будущего внешнего обновления."
        )
        adesc.setWordWrap(True); adesc.setStyleSheet("color:#555;")
        agl.addWidget(adesc); ly.addWidget(ag)

        self._agent_sync_local(quiet=True)
        self._agent_refresh_status()

    @staticmethod
    def _creds(rt):
        m={"sftp":[("sftp_u","Пользователь SSH:","user",False),("sftp_p","Пароль SSH:","●●●",True)],
           "s3":  [("s3_k","Access Key ID:","AKIA…",False),("s3_s","Secret Key:","●●●",True)],
           "b2":  [("b2_i","Account ID:","000…",False),("b2_k","App Key:","K0…",True)],
           "azure":[("az_a","Account Name:","storage",False),("az_k","Account Key:","base64",True)],
           "gs":  [("gs_p","Project ID:","my-proj",False),("gs_j","JSON Key File:","key.json",False)],}
        return m.get(rt,[])

    # ─── RETENTION TAB ──────────────────────────────────────────────────────

    def _build_retention(self):
        tab=QWidget(); ly=QVBoxLayout(tab)
        grp=QGroupBox("Политика хранения (привязана к профилю)"); gl=QVBoxLayout(grp)
        gl.addWidget(QLabel("0 = не использовать параметр"))
        self._sp={}
        for k,lbl in [("keep-last","Хранить последние N копий (--keep-last):"),
                      ("keep-daily","По одной за N дней (--keep-daily):"),
                      ("keep-weekly","По одной за N недель (--keep-weekly):"),
                      ("keep-monthly","По одной за N месяцев (--keep-monthly):"),
                      ("keep-yearly","По одной за N лет (--keep-yearly):")]:
            row=QHBoxLayout(); l=QLabel(lbl); l.setMinimumWidth(340)
            sp=QSpinBox(); sp.setRange(0,9999); sp.setFixedWidth(80)
            sp.valueChanged.connect(self._changed)
            sp.valueChanged.connect(lambda _v: (self._refresh_prune_column(), self._sched_render_preview_cmd()))
            self._sp[k]=sp; row.addWidget(l); row.addWidget(sp); row.addStretch()
            gl.addLayout(row)
        ly.addWidget(grp)

        bc=QPushButton("🧹 Запустить очистку репозитория")
        bc.setFixedHeight(24)
        bc.clicked.connect(self._run_clean_manual)
        ly.addWidget(bc); ly.addStretch()
        self.tabs.addTab(tab,"📦 Хранение")

    # ─── LOGS TAB ────────────────────────────────────────────────────────────

    def _build_logs(self):
        tab=QWidget(); ly=QVBoxLayout(tab)
        self._tab_logs = tab
        self.log_tabs=QTabWidget()
        self.log_info_view=QPlainTextEdit(); self.log_info_view.setReadOnly(True); self.log_info_view.setFont(QFont("Consolas",9))
        self.log_debug_view=QPlainTextEdit(); self.log_debug_view.setReadOnly(True); self.log_debug_view.setFont(QFont("Consolas",9))
        self.log_error_view=QPlainTextEdit(); self.log_error_view.setReadOnly(True); self.log_error_view.setFont(QFont("Consolas",9))
        for view in (self.log_info_view, self.log_debug_view, self.log_error_view):
            view.setUndoRedoEnabled(False)
            view.setLineWrapMode(QPlainTextEdit.NoWrap)
            view.document().setMaximumBlockCount(MAX_LOG_BLOCKS)
        self.log_tabs.addTab(self.log_info_view,"Info")
        self.log_tabs.addTab(self.log_error_view,"Error")
        ly.addWidget(self.log_tabs)
        br = QHBoxLayout()
        bc = QPushButton("🗑 Очистить логи"); bc.clicked.connect(self._clr_logs)
        bx = QPushButton("➕ Добавить проблемные файлы в исключения")
        bx.setToolTip("Парсит ошибки из вкладки Error, добавляет пути из них в исключения текущего профиля (без дубликатов).")
        bx.clicked.connect(self._add_failed_files_to_exclude)
        br.addWidget(bc); br.addWidget(bx); br.addStretch()
        ly.addLayout(br)
        self.tabs.addTab(tab,"📋 Логи")

    # ─── INTEGRATIONS TAB ────────────────────────────────────────────────────

    def _build_integrations(self):
        tab=QWidget(); ly=QVBoxLayout(tab)
        note=QLabel("⚠️ Глобальная вкладка — не привязана к профилю")
        note.setStyleSheet("color:#885500;font-style:italic;"); ly.addWidget(note)
        tg=QGroupBox("Telegram"); tgl=QVBoxLayout(tg)
        r1=QHBoxLayout(); r1.addWidget(QLabel("API Token:"))
        self.ed_tgt=QLineEdit(self.cfg.tg_token); self.ed_tgt.setEchoMode(QLineEdit.Password)
        self.ed_tgt.editingFinished.connect(self._save_tg)
        r1.addWidget(self.ed_tgt,1); tgl.addLayout(r1)
        r2=QHBoxLayout(); r2.addWidget(QLabel("ID чата:"))
        self.ed_tgc=QLineEdit(self.cfg.tg_chat); self.ed_tgc.setPlaceholderText("-100XXXXXXXXXX"); self.ed_tgc.editingFinished.connect(self._save_tg); r2.addWidget(self.ed_tgc,1); tgl.addLayout(r2)
        r2g=QHBoxLayout(); r2g.addWidget(QLabel("Тема (topic ID):"))
        self.ed_tg_group=QLineEdit(self.cfg.tg_group); self.ed_tg_group.setPlaceholderText("793 (опционально)"); self.ed_tg_group.editingFinished.connect(self._save_tg); r2g.addWidget(self.ed_tg_group,1); tgl.addLayout(r2g)
        rb=QHBoxLayout()
        bt=QPushButton("📡 Проверить"); bt.clicked.connect(self._test_tg)
        self.lbl_tgs=QLabel("")
        rb.addWidget(bt); rb.addWidget(self.lbl_tgs); rb.addStretch()
        tgl.addLayout(rb); ly.addWidget(tg)

        rg=QGroupBox("Веб-отчёты и обновление службы"); rgl=QVBoxLayout(rg)
        rr1=QHBoxLayout(); rr1.addWidget(QLabel("Report URL:"))
        self.ed_report_url=QLineEdit(self.cfg.report_url)
        self.ed_report_url.setPlaceholderText("https://example.com/api/restlex/report")
        rr1.addWidget(self.ed_report_url,1); rgl.addLayout(rr1)
        rr2=QHBoxLayout(); rr2.addWidget(QLabel("Update URL:"))
        self.ed_agent_update_url=QLineEdit(self.cfg.agent_auto_update_url)
        self.ed_agent_update_url.setPlaceholderText("https://example.com/restlex-agent.exe")
        rr2.addWidget(self.ed_agent_update_url,1); rgl.addLayout(rr2)
        rdesc=QLabel(
            "Report URL получает JSON со статусом backup. "
            "Update URL пока только сохраняется в конфиге как задел под Rust-службу."
        )
        rdesc.setWordWrap(True); rdesc.setStyleSheet("color:#555;")
        rgl.addWidget(rdesc); ly.addWidget(rg)

        ag=QGroupBox("Служба RestlexAgent"); agl=QVBoxLayout(ag)
        arow=QHBoxLayout()
        self.lbl_agent_status=QLabel("—")
        self.lbl_agent_status.setStyleSheet("color:gray;font-weight:bold;")
        arow.addWidget(QLabel("Статус:"))
        arow.addWidget(self.lbl_agent_status)
        arow.addStretch()
        agl.addLayout(arow)
        abtn=QHBoxLayout()
        for lbl,fn,w in [
            ("➕ Установить и запустить", self._agent_install_and_start, 190),
            ("⏹ Остановить", self._agent_stop, 120),
            ("🗑 Удалить", self._agent_uninstall, 120),
        ]:
            b=QPushButton(lbl); b.setFixedWidth(w); b.clicked.connect(fn); abtn.addWidget(b)
        abtn.addStretch()
        agl.addLayout(abtn)
        adesc=QLabel("Фоновая служба Windows для расписаний, веб-отчётов и управления серверами.")
        adesc.setWordWrap(True); adesc.setStyleSheet("color:#555;")
        agl.addWidget(adesc); ly.addWidget(ag)

        self._agent_sync_local(quiet=True)
        self._agent_refresh_status()

        ly.addStretch()
        self.tabs.addTab(tab,"🔗 Интеграции")

    # ─── PROFILE MANAGEMENT ──────────────────────────────────────────────────

    def _reload_profiles(self):
        self.plist.clear()
        for pid,pd in sorted(self.cfg.profiles().items(), key=lambda item: (item[1].get("name","").lower(), item[0])):
            it=QListWidgetItem(pd.get("name","(без имени)"))
            it.setData(Qt.UserRole,pid); self.plist.addItem(it)
        self._plist_refresh_sched()

    def _sched_short_label(self, sched: dict):
        """Короткое время запуска для списка профилей: (текст, серым?)."""
        sched = sched or {}
        freq = str(sched.get("freq", "") or "")
        times = ", ".join(sorted(set(str(t) for t in (sched.get("times") or [sched.get("time", "02:00")]) if t)))
        if freq == "daily":
            text = times
        elif freq == "weekly":
            codes = [c.strip().upper() for c in str(sched.get("days", "MON")).split(",") if c.strip()]
            days = "Пн-Пт" if codes == ["MON", "TUE", "WED", "THU", "FRI"] else ",".join(self._sched_day_label(c) for c in codes)
            text = f"{times} {days}"
        elif freq == "hourly":
            text = f"каждые {int(sched.get('interval', 4) or 4)} ч"
        elif freq == "onlogon":
            text = "при входе"
        else:
            return "—", True
        if sched.get("enabled", True) is False:  # пауза из Recondeck/API
            return f"⏸ {text}", True
        return text, False

    def _plist_refresh_sched(self):
        profs = self.cfg.profiles()
        for i in range(self.plist.count()):
            it = self.plist.item(i)
            sched = (profs.get(it.data(Qt.UserRole)) or {}).get("sched") or {}
            text, dim = self._sched_short_label(sched)
            it.setData(Qt.UserRole + 1, text); it.setData(Qt.UserRole + 2, dim)
            it.setToolTip(self._sched_when_label(sched) if sched else "Нет расписания")
        self.plist.viewport().update()

    def _prof_ctx(self, pos: QPoint):
        item = self.plist.itemAt(pos)
        menu = QMenu(self)
        if item:
            self.plist.setCurrentItem(item)
            self._on_prof_click(item)
            a_ren = menu.addAction("✏️ Переименовать")
            a_ren.triggered.connect(lambda _, it=item: self._rename_prof(it))
            a_dup = menu.addAction("📄 Копировать профиль")
            a_dup.triggered.connect(self._dup_prof)
            a_del = menu.addAction("🗑 Удалить профиль")
            a_del.triggered.connect(self._del_prof)
            menu.addSeparator()
            a_exp = menu.addAction("⬆ Экспорт профиля")
            a_exp.triggered.connect(self._export_prof)
        a_new = menu.addAction("➕ Новый профиль")
        a_new.triggered.connect(self._add_prof)
        a_imp = menu.addAction("⬇ Импорт профиля")
        a_imp.triggered.connect(self._import_prof)
        menu.addSeparator()
        a_exp_all = menu.addAction("📦 Экспорт всего")
        a_exp_all.triggered.connect(self._export_all)
        a_imp_all = menu.addAction("📥 Импорт всего")
        a_imp_all.triggered.connect(self._import_all)
        menu.exec_(self.plist.viewport().mapToGlobal(pos))

    def _upd_title(self):
        self.setWindowTitle(APP_TITLE)

    def _running_backups(self) -> list:
        """Все идущие бэкапы на этом ПК, независимо от источника запуска:
        агент (планировщик/REST) + любой GUI, включая другой экземпляр."""
        import time as _t
        if _t.monotonic() - SchedulerManager._status_cache.get("ts", 0.0) > 4.0:
            SchedulerManager._refresh_status_cache()
        out = {}
        for pid in SchedulerManager._status_cache.get("running_now", []):
            prof = self.cfg.profile(pid) or {}
            try:
                started = datetime.fromtimestamp((Config.LOGS / f"{pid}.running").stat().st_mtime)
            except Exception:
                started = None
            out[pid] = {"pid": pid, "name": prof.get("name", pid),
                        "started": started, "src": "агент"}
        try:
            by_name = {v.get("name", ""): k for k, v in self.cfg.profiles().items()}
            for f in BackupHeartbeat.DIR.glob("*.json"):
                try:
                    d = json.loads(f.read_text(encoding="utf-8"))
                    # ponytail: heartbeat пишется раз в 60 с — 5 мин тишины считаем
                    # мёртвым процессом, файл после падения никто не убирает
                    if (datetime.now() - datetime.fromisoformat(d["last_seen"])).total_seconds() > 300:
                        continue
                    name = d.get("profile") or "—"
                    pid = by_name.get(name, f.stem)
                    try:
                        started = datetime.fromisoformat(d.get("started", ""))
                    except Exception:
                        started = None
                    out.setdefault(pid, {"pid": pid, "name": name,
                                         "started": started, "src": "GUI"})
                except Exception:
                    continue
        except Exception:
            pass
        return sorted(out.values(), key=lambda r: str(r["name"]).lower())

    def _refresh_global_bk_banner(self):
        lbl = getattr(self, "lbl_global_bk_banner", None)
        if lbl is None:
            return
        runs = self._running_backups()
        self._running_bk = runs
        if not runs:
            lbl.setVisible(False)
            return

        def _one(r):
            if not r["started"]:
                return f'{r["name"]} [{r["src"]}]'
            mins = max(0, int((datetime.now() - r["started"]).total_seconds() // 60))
            return f'{r["name"]} [{r["src"]}, {mins} мин]'

        head = "⏳ Идёт бэкап: " if len(runs) == 1 else f"⏳ Идёт бэкапов: {len(runs)} — "
        lbl.setText(head + ";   ".join(_one(r) for r in runs) + "   (клик — перейти)")
        lbl.setToolTip("\n".join(_one(r) for r in runs))
        lbl.setVisible(True)

    def _jump_to_running_backup(self):
        runs = getattr(self, "_running_bk", None) or []
        if not runs:
            return
        target = runs[0]["pid"]
        for i in range(self.plist.count()):
            it = self.plist.item(i)
            if it.data(Qt.UserRole) == target:
                self.plist.setCurrentRow(i)
                self._on_prof_click(it)
                break

    def _current_tab_is(self, widget) -> bool:
        return bool(widget) and getattr(self, "tabs", None) is not None and self.tabs.currentWidget() is widget

    def _is_restore_tab_active(self) -> bool:
        return self._current_tab_is(getattr(self, "_tab_restore", None))

    def _is_logs_tab_active(self) -> bool:
        return self._current_tab_is(getattr(self, "_tab_logs", None))

    def _is_backup_tab_active(self) -> bool:
        return self._current_tab_is(getattr(self, "_tab_backup", None))

    def _is_scheduler_tab_active(self) -> bool:
        return self._current_tab_is(getattr(self, "_tab_scheduler", None))

    def _repo_field_keys(self, rt: str) -> List[str]:
        if rt == "rest":
            return ["rest_host", "rest_path", "rest_user", "rest_pass", "rest_scheme"]
        return [c[0] for c in self._creds(rt)]

    def _all_repo_field_keys(self) -> List[str]:
        keys = ["repo_type", "repo_path", "repo_password"]
        for rt, *_ in REPO_TYPES:
            keys.extend(self._repo_field_keys(rt))
        return sorted(set(keys))

    def _repo_data_from_ui(self) -> dict:
        rt = self._rtype()
        pg = self._rpages.get(rt, {})
        if rt == "rest":
            repo_path = pg.get("rest_url", QLineEdit()).text().strip()
            extra = {k: pg.get(k, QLineEdit()).text() for k in self._repo_field_keys(rt) if k in pg}
        else:
            repo_path = pg.get("addr", QLineEdit()).text().strip()
            extra = {k: pg.get(k, QLineEdit()).text() for k in self._repo_field_keys(rt) if k in pg}
        data = {
            "repo_type": rt,
            "repo_path": repo_path,
            "repo_password": self.edit_pw.text(),
            "wol_enabled": self.chk_wol.isChecked(),
            "wol_mac": normalize_mac(self.edit_wol_mac.text()) or self.edit_wol_mac.text().strip(),
            "wol_host": self.edit_wol_host.text().strip(),
            "wol_ping_interval": int(self.spin_wol_interval.value()),
            "wol_wait": self.chk_wol_wait.isChecked(),
            "wol_wait_timeout": int(self.spin_wol_timeout.value()),
            "shutdown_after_backup": self.chk_shutdown_after.isChecked(),
            "shutdown_delay": int(self.spin_shutdown_delay.value()),
            "shutdown_remote": self.chk_shutdown_remote.isChecked(),
            "shutdown_remote_mac": normalize_mac(self.edit_shutdown_mac.text()) or self.edit_shutdown_mac.text().strip(),
        }
        data.update(extra)
        return data

    def _repo_data_from_profile(self, prof: Optional[dict] = None) -> dict:
        prof = prof or (self.cfg.profile(self.pid) or {})
        rt = str(prof.get("repo_type", "local") or "local")
        data = {
            "repo_type": rt,
            "repo_path": str(prof.get("repo_path", "") or ""),
            "repo_password": str(prof.get("repo_password", "") or ""),
            "wol_enabled": bool(prof.get("wol_enabled", False)),
            "wol_mac": str(prof.get("wol_mac", "") or ""),
            "wol_host": str(prof.get("wol_host", "") or ""),
            "wol_ping_interval": int(prof.get("wol_ping_interval", WOL_PING_INTERVAL_DEF) or WOL_PING_INTERVAL_DEF),
            "wol_wait": bool(prof.get("wol_wait", True)),
            "wol_wait_timeout": int(prof.get("wol_wait_timeout", WOL_WAIT_TIMEOUT_DEF) or WOL_WAIT_TIMEOUT_DEF),
            "shutdown_after_backup": bool(prof.get("shutdown_after_backup", False)),
            "shutdown_delay": int(prof.get("shutdown_delay", SHUTDOWN_DELAY_DEF)),
            "shutdown_remote": bool(prof.get("shutdown_remote", False)),
            "shutdown_remote_mac": str(prof.get("shutdown_remote_mac", "") or ""),
        }
        for key in self._repo_field_keys(rt):
            data[key] = str(prof.get(key, "") or "")
        return data

    def _repo_changed(self):
        self._repo_dirty = True
        if hasattr(self, "btn_repo_save"):
            self.btn_repo_save.setEnabled(True)
        self._changed()

    def _mark_repo_saved(self):
        self._repo_dirty = False
        if hasattr(self, "btn_repo_save"):
            self.btn_repo_save.setEnabled(False)

    def _sync_encrypt_ui(self):
        """Привести UI секретов в соответствие с тумблером encrypt_secrets:
        при ON — поля секретов принудительно в виде точек, 'глазки' выключены
        (показать значение нельзя); при OFF — 'глазки' доступны. Галку можно
        переключать в обе стороны (с предупреждениями в _on_encrypt_toggle)."""
        on = self.cfg.encrypt_secrets_enabled()
        if hasattr(self, "chk_encrypt"):
            self.chk_encrypt.blockSignals(True)
            self.chk_encrypt.setChecked(on)
            self.chk_encrypt.blockSignals(False)
        for ed_name, eye_name in (("edit_pw", "_pw_eye_btn"), ("ed_update_key", "_upd_key_eye")):
            eye = getattr(self, eye_name, None)
            ed = getattr(self, ed_name, None)
            if eye is not None:
                if on:
                    eye.setChecked(False)
                    eye.setEnabled(False)
                else:
                    eye.setEnabled(True)
            if ed is not None and on:
                ed.setEchoMode(QLineEdit.Password)

    def _on_encrypt_toggle(self, on: bool):
        if on == self.cfg.encrypt_secrets_enabled():
            return
        if on:
            r = QMessageBox.warning(
                self, "Включить шифрование секретов",
                "Зашифровать все секреты на диске через DPAPI "
                "(пароли репозиториев, Telegram-токен, API-ключи)?\n\n"
                "После включения значения секретов нельзя будет показать в интерфейсе. "
                "Бэкапы продолжат работать (агент расшифровывает пароль для restic).",
                QMessageBox.Yes | QMessageBox.No, QMessageBox.No)
            if r != QMessageBox.Yes:
                self._sync_encrypt_ui(); return
            # Раньше флаг включался безусловно и UI рапортовал «зашифровано», а
            # _dpapi_encrypt при сбое молча возвращал ПЛЕЙНТЕКСТ. Проверяем DPAPI
            # ДО включения флага и честно сообщаем, если шифрование недоступно.
            try:
                _dpapi_probe()
            except Exception as e:
                self._sync_encrypt_ui()
                QMessageBox.critical(
                    self, "Шифрование секретов",
                    f"DPAPI недоступен на этой машине ({e}).\n\n"
                    f"Шифрование НЕ включено — секреты остались открытым текстом. "
                    f"Иначе они всё равно писались бы плейнтекстом, а интерфейс "
                    f"утверждал бы обратное.")
                return
            new_flag = True
            done_msg = "Включено. Секреты зашифрованы на диске (DPAPI)."
        else:
            r = QMessageBox.warning(
                self, "⚠️ Отключить шифрование секретов?",
                "ВНИМАНИЕ! Все секреты (пароли репозиториев, Telegram-токен, API-ключи) "
                "будут РАСШИФРОВАНЫ и сохранены ОТКРЫТЫМ ТЕКСТОМ на диске в "
                "C:\\ProgramData\\RestLex\\. Любой, у кого есть доступ к этим файлам, "
                "увидит пароли.\n\n"
                "Кроме того, если какой-то секрет был зашифрован на ДРУГОЙ машине "
                "(перенос конфига) и не расшифровывается здесь — после отключения он "
                "может стать нерабочим, и доступ к такому репозиторию сломается.\n\n"
                "Отключать только если осознаёте риск. Продолжить?",
                QMessageBox.Yes | QMessageBox.No, QMessageBox.No)
            if r != QMessageBox.Yes:
                self._sync_encrypt_ui(); return
            new_flag = False
            done_msg = "Отключено. Секреты сохранены открытым текстом на диске."
        # reencrypt_all перечитывает файлы с диска перед записью (иначе откатывали
        # бы правки агента/Recondeck, сделанные за время работы GUI) и возвращает
        # False, если запись не удалась. Флаг encrypt_secrets выставляется ВНУТРИ
        # (а не отдельным sg() выше): иначе при сбое записи profiles.json флаг
        # оставался ON поверх плейнтекстовых профилей и все save_profile падали.
        if not self.cfg.reencrypt_all(new_flag):
            self._sync_encrypt_ui()
            return
        try:
            RestServerManager._agent_call("POST", "/reload")   # агент сразу подхватит флаг
        except Exception:
            pass
        self._sync_encrypt_ui()
        QMessageBox.information(self, "Шифрование секретов", done_msg)

    def _load_repo_fields(self, prof: Optional[dict] = None):
        prof = prof or (self.cfg.profile(self.pid) or {})
        repo = self._repo_data_from_profile(prof)
        rt = repo.get("repo_type", "local")
        ridx = RIDX.get(rt, 0)
        self.rtype_list.blockSignals(True)
        self.rtype_list.setCurrentRow(ridx)
        self.rtype_list.blockSignals(False)
        self.rstack.setCurrentIndex(ridx)
        if rt == "rest":
            pg = self._rpages.get("rest", {})
            for key in self._repo_field_keys("rest"):
                widget = pg.get(key)
                if widget:
                    widget.setText(repo.get(key, ""))
            # миграция профилей, сохранённых до появления rest_scheme: схему берём из repo_path
            w_sch = pg.get("rest_scheme")
            if w_sch and not w_sch.text().strip():
                old = str(repo.get("repo_path", "") or "")
                if old.startswith("rest:"): old = old[5:]
                w_sch.setText(urllib.parse.urlparse(old).scheme.lower() or "http")
            self._upd_rest_url()
        else:
            pg = self._rpages.get(rt, {})
            addr = pg.get("addr")
            if addr:
                addr.setText(repo.get("repo_path", ""))
            for key in self._repo_field_keys(rt):
                widget = pg.get(key)
                if widget:
                    widget.setText(repo.get(key, ""))
        self.edit_pw.setText(repo.get("repo_password", ""))
        self._power_loading = True
        self.chk_wol.setChecked(bool(repo.get("wol_enabled", False)))
        self.edit_wol_mac.setText(str(repo.get("wol_mac", "") or ""))
        self.edit_wol_host.setText(str(repo.get("wol_host", "") or ""))
        self.spin_wol_interval.setValue(int(repo.get("wol_ping_interval", WOL_PING_INTERVAL_DEF) or WOL_PING_INTERVAL_DEF))
        self.chk_wol_wait.setChecked(bool(repo.get("wol_wait", True)))
        self.spin_wol_timeout.setValue(int(repo.get("wol_wait_timeout", WOL_WAIT_TIMEOUT_DEF) or WOL_WAIT_TIMEOUT_DEF))
        self.spin_wol_timeout.setEnabled(self.chk_wol_wait.isChecked())
        self._wol_sub.setVisible(self.chk_wol.isChecked())
        self.chk_shutdown_after.setChecked(bool(repo.get("shutdown_after_backup", False)))
        self.spin_shutdown_delay.setValue(int(repo.get("shutdown_delay", SHUTDOWN_DELAY_DEF)))
        self.chk_shutdown_remote.setChecked(bool(repo.get("shutdown_remote", False)))
        self.edit_shutdown_mac.setText(str(repo.get("shutdown_remote_mac", "") or ""))
        self.lbl_wol_status.setVisible(False); self.lbl_sd_status.setVisible(False)
        self._power_loading = False
        self._val_addr()
        self._mark_repo_saved()
        if hasattr(self, "lbl_chk"):
            self.lbl_chk.clear()

    def _discard_repo_changes(self):
        self._load_repo_fields(self.cfg.profile(self.pid) or {})

    def _confirm_discard_repo_changes(self, message: str) -> bool:
        if not self._repo_dirty:
            return True
        answer = QMessageBox.question(
            self,
            "Репозиторий",
            message,
            QMessageBox.Yes | QMessageBox.No,
            QMessageBox.No,
        )
        if answer == QMessageBox.Yes:
            self._discard_repo_changes()
            return True
        return False

    def _save_repo(self) -> bool:
        if not self.pid:
            if hasattr(self, "lbl_chk"):
                self.lbl_chk.setText("❌ Выберите профиль")
                self.lbl_chk.setStyleSheet("color:#cc0000;")
            return False
        data = self._collect(include_repo=True)
        if not data:
            return False
        self.cfg.save_profile(self.pid, data)
        self._mark_repo_saved()
        if hasattr(self, "lbl_chk"):
            self.lbl_chk.setText("✅ Репозиторий сохранён")
            self.lbl_chk.setStyleSheet("color:#006600;")
        self._saved()
        return True

    def _show_loading(self, text="Загрузка…"):
        # Модальное окно блокировало переключение вкладок и могло зависнуть — только строка состояния.
        self.statusBar().showMessage(text, 0)

    def _hide_loading(self):
        self.statusBar().clearMessage()

    def _center_dialog(self, dlg: QDialog):
        try:
            geo = dlg.frameGeometry()
            geo.moveCenter(self.frameGeometry().center())
            dlg.move(geo.topLeft())
        except Exception:
            pass

    def _ensure_restore_progress_dialog(self):
        if self._restore_progress_dialog is not None:
            return self._restore_progress_dialog
        dlg = QDialog(self)
        dlg.setWindowTitle("Восстановление")
        dlg.setWindowFlags((dlg.windowFlags() & ~Qt.WindowContextHelpButtonHint) | Qt.WindowStaysOnTopHint)
        dlg.resize(760, 340)
        ly = QVBoxLayout(dlg)
        self._restore_progress_status_lbl = QLabel("Восстанавливаю…")
        self._restore_progress_status_lbl.setStyleSheet("font-size:16px;font-weight:bold;")
        self._restore_progress_bar = QProgressBar()
        self._restore_progress_bar.setTextVisible(True)
        self._restore_progress_bar.setRange(0, 0)
        self._restore_progress_file_lbl = QLabel("")
        self._restore_progress_file_lbl.setWordWrap(True)
        self._restore_progress_file_lbl.setStyleSheet("color:#333;background:#f7f7f7;border:1px solid #d5d5d5;padding:8px;")
        self._restore_progress_log = QPlainTextEdit()
        self._restore_progress_log.setReadOnly(True)
        self._restore_progress_log.setUndoRedoEnabled(False)
        self._restore_progress_log.setLineWrapMode(QPlainTextEdit.NoWrap)
        self._restore_progress_log.document().setMaximumBlockCount(200)
        ly.addWidget(self._restore_progress_status_lbl)
        ly.addWidget(self._restore_progress_bar)
        ly.addWidget(self._restore_progress_file_lbl)
        ly.addWidget(self._restore_progress_log, 1)
        btn_row = QHBoxLayout()
        btn_hide = QPushButton("Свернуть в фон")
        btn_hide.setToolTip("Скрыть окно — восстановление продолжится в фоне. Прогресс виден в строке состояния.")
        btn_hide.clicked.connect(dlg.hide)
        btn_row.addStretch(); btn_row.addWidget(btn_hide)
        ly.addLayout(btn_row)
        self._restore_progress_dialog = dlg
        return dlg

    def _show_restore_progress(self, status: str, current: str = "", maximum: int = 0, value: int = 0, clear: bool = False):
        dlg = self._ensure_restore_progress_dialog()
        self._restore_progress_status_lbl.setText(status)
        if maximum > 0:
            if self._restore_progress_bar.maximum() != maximum:
                self._restore_progress_bar.setRange(0, maximum)
            self._restore_progress_bar.setValue(max(0, min(value, maximum)))
        else:
            self._restore_progress_bar.setRange(0, 0)
        if clear:
            self._restore_progress_log.clear()
        self._restore_progress_file_lbl.setText(current or "Подождите…")
        dlg.show()
        dlg.raise_()
        dlg.activateWindow()
        self._center_dialog(dlg)
        QApplication.processEvents()

    def _append_restore_progress(self, line: str, current: str = ""):
        # Виджеты — только UI-поток; из воркеров (restore-thread) — queued-сигнал.
        if threading.current_thread() is not threading.main_thread():
            self._ui_call_signal.emit(lambda: self._append_restore_progress(line, current))
            return
        dlg = self._ensure_restore_progress_dialog()
        if line:
            self._restore_progress_log.appendPlainText(line)
        if current:
            self._restore_progress_file_lbl.setText(current)
        if not dlg.isVisible():
            dlg.show()
            self._center_dialog(dlg)
        QApplication.processEvents()

    def _set_restore_progress(self, status: Optional[str] = None, current: Optional[str] = None, maximum: Optional[int] = None, value: Optional[int] = None):
        if self._restore_progress_dialog is None:
            return
        if status is not None:
            self._restore_progress_status_lbl.setText(status)
        if maximum is not None:
            if maximum > 0:
                if self._restore_progress_bar.maximum() != maximum:
                    self._restore_progress_bar.setRange(0, maximum)
            else:
                self._restore_progress_bar.setRange(0, 0)
        if value is not None and self._restore_progress_bar.maximum() > 0:
            self._restore_progress_bar.setValue(max(0, min(value, self._restore_progress_bar.maximum())))
        if current is not None:
            self._restore_progress_file_lbl.setText(current)
        QApplication.processEvents()

    def _hide_restore_progress(self):
        if self._restore_progress_dialog is not None:
            self._restore_progress_dialog.hide()
            QApplication.processEvents()

    def _stop_worker(self, worker: Optional[ResticWorker]):
        if not worker:
            return
        try:
            worker.stop()
        except Exception:
            pass

    def _shutdown_runtime(self):
        if self._closing:
            return
        self._closing = True
        # штатное закрытие GUI — watchdog выключения не должен слать уведомление
        ev = getattr(self, "_shutdown_watch_ev", None)
        if ev is not None:
            ev.set()
        try:
            self._hide_loading()
        except Exception:
            pass
        try:
            self._hide_restore_progress()
        except Exception:
            pass
        for timer_name in ("_chk_timer", "_snap_browser_timer", "_restore_timer",
                           "_search_poll_timer", "_search_debounce", "_bk_banner_timer",
                           # Эти тикали по уже разрушенному окну →
                           # «RuntimeError: wrapped C/C++ object has been deleted».
                           "_log_flush_timer", "_service_badge_timer", "_sched_auto_timer",
                           "_res_monitor_timer", "_backup_timeout_timer"):
            timer = getattr(self, timer_name, None)
            if timer is not None:
                try:
                    timer.stop()
                except Exception:
                    pass
        # Фоновое восстановление: гасим restic, иначе после закрытия окна он
        # продолжает писать поверх боевых данных без UI и контроля.
        for proc_name in ("_restore_proc", "_search_proc"):
            proc = getattr(self, proc_name, None)
            if proc is not None:
                try:
                    proc.kill()
                except Exception:
                    pass
                setattr(self, proc_name, None)
        workers = []
        seen = set()
        for worker in [*self._workers.values(), self._snapshots_worker, self._ls_worker, self._chk_worker, *self._snap_workers.values()]:
            if not worker:
                continue
            key = id(worker)
            if key in seen:
                continue
            seen.add(key)
            workers.append(worker)
        for worker in workers:
            self._stop_worker(worker)
        self._workers.clear()
        self._snapshots_worker = None
        self._ls_worker = None
        self._chk_worker = None
        self._cancel_deleted_scan()
        self._snap_workers.clear()
        try:
            self._close_mount_explorer_windows()
        except Exception:
            pass
        try:
            self._close_snap_browser()
        except Exception:
            pass
        try:
            self._stop_mount_quiet(fast=True)
        except Exception:
            pass

    def _close_mount_explorer_windows(self, unc: str = ""):
        base_unc = self._normalize_win_path(unc or getattr(self, "_snap_browser_unc_root", "") or "")
        if not base_unc and hasattr(self, "ed_mount_path"):
            base_unc = self._normalize_win_path(self.ed_mount_path.text().strip())
        if not base_unc:
            return
        base_unc = base_unc.rstrip("\\/").lower()
        for window in self._shell_windows():
            try:
                path = self._shell_window_path(window).rstrip("\\/").lower()
            except Exception:
                path = ""
            if not path or not (path == base_unc or path.startswith(base_unc + "\\")):
                continue
            try:
                window.Quit()
                continue
            except Exception:
                pass
            try:
                hwnd = int(getattr(window, "HWND", 0) or 0)
                if hwnd:
                    win32gui.PostMessage(hwnd, win32con.WM_CLOSE, 0, 0)
            except Exception:
                pass

    def _on_prof_click(self,item):
        new_pid=item.data(Qt.UserRole)
        if self.pid and new_pid != self.pid and self._current_tab_is(getattr(self, "_tab_repo", None)) and self._repo_dirty:
            if not self._confirm_discard_repo_changes("Есть несохранённые изменения репозитория.\nОтменить их и переключить профиль?"):
                self.plist.blockSignals(True)
                for i in range(self.plist.count()):
                    cur = self.plist.item(i)
                    if cur.data(Qt.UserRole) == self.pid:
                        self.plist.setCurrentRow(i)
                        break
                self.plist.blockSignals(False)
                return
        keep_loading = self._is_restore_tab_active()
        self._show_loading("Загрузка профиля…")
        if self.pid and new_pid != self.pid:
            self._stop_mount_quiet(fast=True)
        self._autosave(include_repo=not self._repo_dirty)
        self.pid=new_pid
        prof=self.cfg.profile(self.pid) or {}
        self._load_prof(prof)
        if not keep_loading:
            self._hide_loading()

    def _load_prof(self,prof):
        def _clean(s: str) -> str:
            return s.lstrip("﻿").strip().replace("/", "\\")
        self.src_list.clear(); self.src_list.addItems([_clean(s) for s in prof.get("sources", []) if _clean(s)])
        self.excl_list.clear(); self.excl_list.addItems([_clean(s) for s in prof.get("exclude", []) if _clean(s)])
        self._load_repo_fields(prof)
        self.chk_vss.setChecked(prof.get("use_vss",True))
        ret=prof.get("retention",{}) or {}
        # Self-heal: подхватить ключи с подчёркиванием (старый формат Rust agent)
        for dashed in ("keep-last","keep-hourly","keep-daily",
                       "keep-weekly","keep-monthly","keep-yearly"):
            underscored = dashed.replace("-", "_")
            if underscored in ret and dashed not in ret:
                ret[dashed] = ret[underscored]
        # Защита от ложных valueChanged-эмиссий при смене профиля
        for k,sp in self._sp.items():
            sp.blockSignals(True)
            try:
                sp.setValue(int(ret.get(k, 0) or 0))
            finally:
                sp.blockSignals(False)
        prof=self.cfg.profile(self.pid) or {}
        self.chk_tf.setChecked(prof.get("tg_fatal", self.cfg.tg_fatal))
        self.chk_te.setChecked(prof.get("tg_file", self.cfg.tg_file))
        self.chk_tp.setChecked(prof.get("tg_prune", self.cfg.tg_prune))
        self.chk_pre_repair.setChecked(bool(prof.get("pre_repair_index", True)))
        self.chk_pre_unlock.setChecked(bool(prof.get("pre_unlock", True)))
        self.chk_force_unlock_orphan.setChecked(bool(prof.get("force_unlock_orphan", True)))
        self.spin_stale_grace.setValue(int(prof.get("stale_grace_minutes", 35) or 35))
        self._refresh_sched_retention_label()
        if not self._backup_running or self._backup_pid != self.pid:
            self.prog_bk.setVisible(False)
            self.btn_bk.setVisible(True)
            self.btn_bk.setEnabled(True)
            self.bk_ctrl_row.setVisible(False)
            self.lbl_bk_vss.setVisible(False)
            self._set_bk_status_line("")
            self._set_bk_result_status("idle")
            self.lbl_bk_file.setText("")
            self.lbl_bk_stats.setText("")
            QTimer.singleShot(0, self._load_last_run_stats)
        else:
            self.prog_bk.setVisible(True)
            self.btn_bk.setVisible(False)
            self.bk_ctrl_row.setVisible(True)
            self.lbl_bk_vss.setVisible(True)
            self._set_bk_vss_status(self._bk_vss_ok)
        self._refresh_global_bk_banner()
        # Снимки сбрасываем только при смене профиля/репозитория: повторный клик по тому же профилю
        # (или перечитывание полей) стирал загруженный список и показывал «Снимки не загружены».
        _rkey = (self.pid, (self.cfg.profile(self.pid) or {}).get("repo_path", ""))
        _same_repo = getattr(self, "_restore_state_key", None) == _rkey
        if not _same_repo:
            self._restore_state_key = _rkey
            self._ref_logs(); self.snap_tree.clear(); self._reset_snaps(loading=self._is_restore_tab_active())
        else:
            self._ref_logs()
            if self._snaps:
                self._update_restore_visibility(True)
            self._hide_loading()
        self._val_addr()
        self._sched_load()
        self._sched_refresh_status()
        if hasattr(self, "_sched_auto_timer"):
            if self.pid:
                self._sched_auto_timer.start()
            else:
                self._sched_auto_timer.stop()
        self._upd_title()
        self.statusBar().clearMessage()
        if self._is_restore_tab_active() and not (_same_repo and self._snaps):
            self._update_restore_visibility(False, loading=True)
            QTimer.singleShot(0, self._activate_restore_tab)

    def _add_prof(self):
        name,ok=QInputDialog.getText(self,"Новый профиль","Имя профиля:")
        if not ok or not name: return
        pid=str(uuid.uuid4())[:8]
        self.cfg.save_profile(pid,{"name":name,"sources":[],
            "repo_type":"local","repo_path":"","use_vss":True,
            "retention":{"keep-last":15,"keep-monthly":2},
            "tg_fatal":True,"tg_file":True,"tg_prune":True,
            "pre_repair_index":True,"pre_unlock":True,
            "force_unlock_orphan":True,"stale_grace_minutes":35,
            "wol_enabled":False})
        self._reload_profiles()
        # Select new profile
        for i in range(self.plist.count()):
            if self.plist.item(i).data(Qt.UserRole)==pid:
                self.plist.setCurrentRow(i); self._on_prof_click(self.plist.item(i)); break

    def _del_prof(self):
        if not self.pid: return
        if QMessageBox.question(self,"Удалить?","Удалить профиль?") != QMessageBox.Yes: return
        self._stop_mount_quiet()
        prof = self.cfg.profile(self.pid) or {}
        # Сначала запись профилей, потом снос расписания: при обратном порядке
        # (и при отказе записи из-за лока/битого конфига) профиль оставался на
        # диске БЕЗ задач планировщика — бэкапы тихо переставали идти по графику.
        if not self.cfg.del_profile(self.pid):
            QMessageBox.warning(
                self, "Удалить профиль",
                "Профиль НЕ удалён: не удалось записать profiles.json.\n"
                "Расписание оставлено без изменений. Повторите попытку.")
            return
        SchedulerManager.delete(self.pid, prof.get("name", self.pid))
        for task in self._sched_profile_entries(prof):
            SchedulerManager.delete_task(self.pid, task)
        self.pid=None; self._reload_profiles(); self._upd_title()

    def _rename_prof(self,item):
        pid=item.data(Qt.UserRole)
        old=item.text()
        new,ok=QInputDialog.getText(self,"Переименовать профиль","Новое имя:",text=old)
        if not ok or not new or new==old: return
        prof=self.cfg.profile(pid) or {}
        prof["name"]=new; self.cfg.save_profile(pid,prof)
        item.setText(new)
        self._upd_title()

    def _dup_prof(self):
        if not self.pid:
            QMessageBox.warning(self,"Копия профиля","Выберите профиль"); return
        prof=self.cfg.profile(self.pid) or {}
        base_name=prof.get("name","Профиль")
        new_name,ok=QInputDialog.getText(self,"Копия профиля","Имя новой копии:",text=f"{base_name} — копия")
        if not ok or not new_name.strip(): return
        new_pid=str(uuid.uuid4())[:8]
        clone=json.loads(json.dumps(prof, ensure_ascii=False))
        clone["name"]=new_name.strip()
        self.cfg.save_profile(new_pid,clone)
        self._reload_profiles()
        for i in range(self.plist.count()):
            if self.plist.item(i).data(Qt.UserRole)==new_pid:
                self.plist.setCurrentRow(i); self._on_prof_click(self.plist.item(i)); break

    def _export_include_secrets(self, what: str) -> bool:
        """Спрашивает, класть ли секреты в экспортный файл. Дефолт — НЕТ."""
        r = QMessageBox.question(
            self, "Экспорт",
            f"Включить секреты в экспорт {what}?\n\n"
            f"Пароли репозиториев, Telegram-токен и API-ключи будут записаны в файл "
            f"ОТКРЫТЫМ ТЕКСТОМ. Для «эталона» на общей шаре выбирайте «Нет».",
            QMessageBox.Yes | QMessageBox.No, QMessageBox.No)
        return r == QMessageBox.Yes

    def _export_prof(self):
        if not self.pid:
            QMessageBox.warning(self,"Экспорт профиля","Выберите профиль"); return
        prof=self.cfg.profile(self.pid) or {}
        safe_name=re.sub(r'[\\/:*?"<>|]+', "_", prof.get("name","profile")).strip("_") or "profile"
        path,_=QFileDialog.getSaveFileName(
            self,
            "Экспорт профиля",
            str(Path.home()/f"{safe_name}.restlex-profile.json"),
            "Restlex Profile (*.restlex-profile.json);;JSON (*.json)"
        )
        if not path: return
        # По умолчанию БЕЗ секретов: экспортный файл обычно кладут на общую шару,
        # а repo_password/пароль в rest-URL уходили туда открытым текстом.
        if not self._export_include_secrets("профиля"):
            prof = _strip_export_secrets({"profiles": {self.pid: prof}})["profiles"][self.pid]
        payload={
            "format":"restlex-profile/v1",
            "exported_at":datetime.now().isoformat(),
            "profile":prof,
        }
        try:
            Path(path).write_text(json.dumps(payload,indent=2,ensure_ascii=False), encoding="utf-8")
            self.statusBar().showMessage("✅ Профиль экспортирован", 4000)
        except Exception as e:
            QMessageBox.warning(self,"Экспорт профиля",f"Не удалось сохранить файл:\n{e}")

    # ─── Drag-and-drop экспортного файла на окно ─────────────────────────────
    @staticmethod
    def _dropped_json(e) -> str:
        md = e.mimeData()
        urls = md.urls() if md.hasUrls() else []
        if len(urls) == 1 and urls[0].isLocalFile():
            f = urls[0].toLocalFile()
            if f.lower().endswith(".json"):
                return f
        return ""

    def dragEnterEvent(self, e):
        (e.acceptProposedAction() if self._dropped_json(e) else e.ignore())

    def dragMoveEvent(self, e):
        (e.acceptProposedAction() if self._dropped_json(e) else e.ignore())

    def dropEvent(self, e):
        path = self._dropped_json(e)
        if not path:
            e.ignore(); return
        e.acceptProposedAction()
        # Модальные окна — после возврата из drop, иначе Проводник висит до их закрытия.
        QTimer.singleShot(0, lambda: self._import_dropped(path))

    def _import_dropped(self, path: str):
        try:
            data = json.loads(Path(path).read_text(encoding="utf-8-sig"))
        except Exception as e:
            QMessageBox.warning(self, "Импорт", f"Не удалось прочитать файл:\n{e}")
            return
        if isinstance(data, dict) and ("global" in data or "profiles" in data):
            n = len(data.get("profiles") or {})
            r = QMessageBox.question(
                self, "Импорт всего конфига",
                f"Импортировать весь конфиг из\n{path}?\n\n"
                f"Глобальные настройки и профили ({n}) будут перезаписаны значениями из файла.",
                QMessageBox.Yes | QMessageBox.No, QMessageBox.No)
            if r == QMessageBox.Yes:
                self._import_all(path)
        else:
            self._import_prof(path)

    def _import_prof(self, path=None):
        if not path:
            path,_=QFileDialog.getOpenFileName(
                self,
                "Импорт профиля",
                str(Path.home()),
                "Restlex Profile (*.restlex-profile.json *.json)"
            )
        if not path: return
        try:
            payload=json.loads(Path(path).read_text(encoding="utf-8-sig"))
            prof=payload.get("profile", payload) if isinstance(payload, dict) else {}
            if not isinstance(prof, dict):
                raise ValueError("некорректный формат файла")
        except Exception as e:
            QMessageBox.warning(self,"Импорт профиля",f"Не удалось прочитать файл:\n{e}")
            return
        imported_name=(prof.get("name") or "Импортированный профиль").strip()
        new_name,ok=QInputDialog.getText(self,"Импорт профиля","Имя нового профиля:",text=imported_name)
        if not ok or not new_name.strip(): return
        new_pid=str(uuid.uuid4())[:8]
        clone=json.loads(json.dumps(prof, ensure_ascii=False))
        clone["name"]=new_name.strip()
        self.cfg.save_profile(new_pid,clone)
        self._reload_profiles()
        for i in range(self.plist.count()):
            if self.plist.item(i).data(Qt.UserRole)==new_pid:
                self.plist.setCurrentRow(i); self._on_prof_click(self.plist.item(i)); break

    def _export_all(self):
        path,_=QFileDialog.getSaveFileName(
            self,"Экспорт всего конфига",
            str(Path.home()/"restlex-config.json"),
            "JSON (*.json)")
        if not path: return
        try:
            data={"global":dict(self.cfg._g),"profiles":dict(self.cfg._pr)}
            # То же, что и у CLI `export-config --no-secrets`: по умолчанию чистим
            # repo_password / tg_token / report_api_key / update_api_key.
            if not self._export_include_secrets("конфига"):
                data = _strip_export_secrets(data)
                data.pop("webserver", None)   # мы его сюда и не клали
            Path(path).write_text(json.dumps(data,indent=2,ensure_ascii=False),encoding="utf-8")
            self.statusBar().showMessage("✅ Весь конфиг экспортирован",4000)
        except Exception as e:
            QMessageBox.warning(self,"Экспорт всего конфига",f"Не удалось сохранить:\n{e}")

    def _import_all(self, path=None):
        if not path:
            path,_=QFileDialog.getOpenFileName(
                self,"Импорт всего конфига",str(Path.home()),"JSON (*.json)")
        if not path: return
        try:
            data=json.loads(Path(path).read_text(encoding="utf-8-sig"))
            if not isinstance(data, dict) or not ("global" in data or "profiles" in data):
                raise ValueError("это не экспорт всего конфига (нет global/profiles)")
            # Экспорт "без секретов" (дефолт) кладёт в JSON пустые строки вместо
            # tg_token/repo_password/etc, а не убирает ключи. Слепой импорт этих
            # пустых значений стирал бы уже настроенные секреты на машине-адресате
            # вместо того чтобы просто их не переносить — пропускаем такие поля.
            if "global" in data:
                for k,v in data["global"].items():
                    if k in _EXPORT_SECRET_GLOBAL_KEYS and not v:
                        continue
                    self.cfg.sg(k,v)
            if "profiles" in data:
                for pid,prof in data["profiles"].items():
                    if isinstance(prof, dict):
                        existing = self.cfg.profile(pid) or {}
                        for k in _EXPORT_SECRET_PROFILE_KEYS:
                            if not prof.get(k) and existing.get(k):
                                prof[k] = existing[k]
                    self.cfg.save_profile(pid,prof)
            self._reload_profiles()
            # Без этого виджеты держат старые значения, и ближайший _save_global
            # (смена вкладки, старт бэкапа) затирал импортированные URL/интеграции.
            self._refresh_global_widgets()
            self.statusBar().showMessage("✅ Весь конфиг импортирован",4000)
        except Exception as e:
            QMessageBox.warning(self,"Импорт всего конфига",f"Не удалось прочитать:\n{e}")

    def _collect(self, include_repo: bool = True) -> Optional[dict]:
        if not self.pid: return None
        # Find the item matching self.pid — not currentItem() which may already be the next profile
        item = None
        for i in range(self.plist.count()):
            it = self.plist.item(i)
            if it.data(Qt.UserRole) == self.pid:
                item = it; break
        if not item: return None
        ret={k:sp.value() for k,sp in self._sp.items() if sp.value()}
        sources=[self.src_list.item(i).text() for i in range(self.src_list.count())]
        excl=[self.excl_list.item(i).text() for i in range(self.excl_list.count())]
        d=dict(self.cfg.profile(self.pid) or {})
        for key in self._all_repo_field_keys():
            d.pop(key, None)
        d.update({"name":item.text(),"sources":sources,"exclude":excl,
           "use_vss":self.chk_vss.isChecked(),"retention":ret,
           "tg_fatal":self.chk_tf.isChecked(),"tg_file":self.chk_te.isChecked(),"tg_prune":self.chk_tp.isChecked(),
           "pre_repair_index": self.chk_pre_repair.isChecked(),
           "pre_unlock": self.chk_pre_unlock.isChecked(),
           "force_unlock_orphan": self.chk_force_unlock_orphan.isChecked(),
           "stale_grace_minutes": int(self.spin_stale_grace.value()),})
        repo_data = self._repo_data_from_ui() if include_repo else self._repo_data_from_profile()
        d.update(repo_data)
        return d

    def _autosave(self, include_repo: bool = True):
        if not self.pid: return
        d=self._collect(include_repo=include_repo)
        if d: self.cfg.save_profile(self.pid,d)

    def _set_restic_status(self, text: str, color: str = "#555"):
        if hasattr(self, "lbl_restic_status"):
            self.lbl_restic_status.setText(text)
            self.lbl_restic_status.setStyleSheet(f"color:{color};")

    def _effective_restic_path(self, raw_value: Optional[str] = None) -> str:
        value = str(raw_value if raw_value is not None else getattr(self, "edit_restic", QLineEdit()).text()).strip()
        if value:
            return value
        managed = self.cfg.managed_restic_path()
        return str(managed) if managed.exists() else "restic"

    def _refresh_restic_status_label(self):
        raw_value = getattr(self, "edit_restic", QLineEdit()).text().strip() if hasattr(self, "edit_restic") else ""
        effective = self._effective_restic_path(raw_value)
        managed = self.cfg.managed_restic_path()
        if raw_value:
            self._set_restic_status(f"Используется указанный путь: {effective}")
            return
        if managed.exists():
            self._set_restic_status(f"По умолчанию будет использован: {managed}")
            return
        path_restic = shutil.which("restic")
        if path_restic:
            self._set_restic_status(f"Путь пустой. Пока будет использован restic из PATH: {path_restic}", "#885500")
            return
        self._set_restic_status(f"Путь пустой. Скачайте restic в {managed} или укажите свой путь.", "#885500")

    def _parse_restic_version(self, text: str) -> str:
        m = re.search(r"\brestic\s+([0-9][^\s]*)", str(text or ""), re.IGNORECASE)
        return m.group(1) if m else ""

    def _get_restic_version(self, binary: str) -> str:
        if not binary:
            return ""
        try:
            proc = subprocess.run(
                [binary, "version"],
                capture_output=True,
                text=True,
                timeout=15,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
        except Exception:
            return ""
        output = "\n".join([proc.stdout or "", proc.stderr or ""]).strip()
        return self._parse_restic_version(output)

    # Последняя версия restic, совместимая со старыми ОС (Server 2008R2 / 2012, Win7/8).
    # restic 0.17+ собран на Go 1.22 и не запускается на Windows < 10 / Server 2016.
    _RESTIC_LEGACY_PIN = "0.16.4"

    @staticmethod
    def _is_legacy_os() -> bool:
        """True если Windows < 10 (major version < 10)."""
        try:
            return sys.getwindowsversion().major < 10
        except Exception:
            return False

    def _preferred_restic_assets(self) -> List[str]:
        arch = (os.environ.get("PROCESSOR_ARCHITEW6432") or os.environ.get("PROCESSOR_ARCHITECTURE") or "").lower()
        if "arm64" in arch:
            preferred = ["windows_arm64.zip", "windows_arm64.exe", "windows_amd64.zip", "windows_amd64.exe"]
        elif "64" in arch:
            preferred = ["windows_amd64.zip", "windows_amd64.exe", "windows_386.zip", "windows_386.exe"]
        else:
            preferred = ["windows_386.zip", "windows_386.exe", "windows_amd64.zip", "windows_amd64.exe"]
        return preferred

    def _fetch_latest_restic_release(self, legacy: bool = False) -> dict:
        if legacy:
            # Возвращаем пинованную версию без обращения к GitHub API
            pin = self._RESTIC_LEGACY_PIN
            arch = (os.environ.get("PROCESSOR_ARCHITEW6432") or os.environ.get("PROCESSOR_ARCHITECTURE") or "").lower()
            if "arm64" in arch:
                sfx = "windows_arm64.zip"
            elif "64" in arch:
                sfx = "windows_amd64.zip"
            else:
                sfx = "windows_386.zip"
            name = f"restic_{pin}_{sfx}"
            url = f"https://github.com/restic/restic/releases/download/v{pin}/{name}"
            return {"version": pin, "asset": {"name": name}, "url": url, "legacy": True}

        gh_headers = {"User-Agent": "RestLex", "Accept": "application/vnd.github+json"}
        # requests тянет certifi (bundled root CA) — обходит битое системное
        # хранилище сертификатов Windows, из-за которого urllib.request падает
        # с CERTIFICATE_VERIFY_FAILED в PyInstaller-сборке.
        if _HAS_REQUESTS:
            resp = _req.get("https://api.github.com/repos/restic/restic/releases/latest",
                             headers=gh_headers, timeout=25)
            resp.raise_for_status()
            payload = resp.json()
        else:
            req = urllib.request.Request(
                "https://api.github.com/repos/restic/restic/releases/latest",
                headers=gh_headers,
            )
            with urllib.request.urlopen(req, timeout=25) as resp:
                payload = json.loads(resp.read().decode("utf-8"))
        assets = payload.get("assets") or []
        selected = None
        for suffix in self._preferred_restic_assets():
            selected = next((asset for asset in assets if str(asset.get("name", "")).lower().endswith(suffix)), None)
            if selected:
                break
        if not selected:
            selected = next(
                (
                    asset for asset in assets
                    if "windows" in str(asset.get("name", "")).lower()
                    and str(asset.get("name", "")).lower().endswith((".zip", ".exe"))
                ),
                None,
            )
        if not selected:
            raise RuntimeError("В последнем релизе не найден файл restic для Windows")
        version = str(payload.get("tag_name") or payload.get("name") or "").strip().lstrip("v")
        url = str(selected.get("browser_download_url") or "").strip()
        if not url:
            raise RuntimeError("GitHub не отдал ссылку на скачивание restic")
        return {"version": version or "неизвестно", "asset": selected, "url": url, "legacy": False}

    def _check_restic_version(self):
        # HTTP к api.github.com (до 25с) — в фоне: под setOverrideCursor в UI-потоке
        # окно замерзало целиком. Результат возвращаем через _ui_call_signal.
        if getattr(self, "_restic_net_busy", False):
            return
        self._restic_net_busy = True
        raw_value = self.edit_restic.text().strip() if hasattr(self, "edit_restic") else ""
        self._set_restic_status("⏳ Проверяю версию restic…", "#555")
        QApplication.setOverrideCursor(Qt.WaitCursor)
        def _do():
            try:
                effective = self._effective_restic_path(raw_value)
                current_version = self._get_restic_version(effective)
                is_legacy = self._is_legacy_os()
                release = self._fetch_latest_restic_release(legacy=is_legacy)
                target_version = release.get("version", "неизвестно")
                legacy_note = " (макс. для вашей ОС)" if is_legacy else ""
                if current_version:
                    if current_version == target_version:
                        res = (f"✅ restic {current_version} актуален{legacy_note}", "#006600")
                    else:
                        res = (f"ℹ Установлен restic {current_version}, доступен "
                               f"{target_version}{legacy_note}", "#885500")
                else:
                    res = (f"ℹ Локальный restic не найден, доступна версия "
                           f"{target_version}{legacy_note}", "#885500")
                err = None
            except Exception as e:
                res, err = None, str(e)
            def _ui():
                self._restic_net_busy = False
                QApplication.restoreOverrideCursor()
                if err is None:
                    self._set_restic_status(res[0], res[1])
                    QMessageBox.information(self, "restic", res[0])
                else:
                    msg = f"Не удалось проверить версию restic:\n{err}"
                    self._set_restic_status(f"❌ {msg}", "#cc0000")
                    QMessageBox.warning(self, "restic", msg)
            self._ui_call_signal.emit(_ui)
        threading.Thread(target=_do, daemon=True).start()

    def _download_restic_target(self) -> Path:
        raw_value = self.edit_restic.text().strip() if hasattr(self, "edit_restic") else ""
        return Path(raw_value) if raw_value else self.cfg.managed_restic_path()

    def _extract_restic_exe(self, archive_path: Path, tmp_exe: Path):
        if archive_path.suffix.lower() != ".zip":
            shutil.copy2(str(archive_path), str(tmp_exe))
            return
        with zipfile.ZipFile(archive_path, "r") as zf:
            exe_name = next(
                (
                    name for name in zf.namelist()
                    if name.lower().endswith(".exe") and "restic" in Path(name).name.lower()
                ),
                "",
            )
            if not exe_name:
                exe_name = next((name for name in zf.namelist() if name.lower().endswith(".exe")), "")
            if not exe_name:
                raise RuntimeError("В архиве не найден exe-файл restic")
            with zf.open(exe_name) as src, open(tmp_exe, "wb") as dst:
                shutil.copyfileobj(src, dst)

    def _download_restic_to(self, target: Path):
        release = self._fetch_latest_restic_release(legacy=self._is_legacy_os())
        target.parent.mkdir(parents=True, exist_ok=True)
        temp_dir = Path(tempfile.mkdtemp(prefix="restlex-restic-"))
        try:
            archive_path = temp_dir / str(release["asset"].get("name") or "restic.zip")
            if _HAS_REQUESTS:
                with _req.get(release["url"], headers={"User-Agent": "RestLex"}, timeout=120, stream=True) as resp:
                    resp.raise_for_status()
                    resp.raw.decode_content = True  # иначе gzip-ответ CDN запишется на диск как есть
                    with open(archive_path, "wb") as out:
                        shutil.copyfileobj(resp.raw, out)
            else:
                request = urllib.request.Request(release["url"], headers={"User-Agent": "RestLex"})
                with urllib.request.urlopen(request, timeout=120) as resp, open(archive_path, "wb") as out:
                    shutil.copyfileobj(resp, out)
            tmp_exe = temp_dir / "restic.exe"
            self._extract_restic_exe(archive_path, tmp_exe)
            backup = target.with_suffix(".old.exe")
            if backup.exists():
                backup.unlink()
            if target.exists():
                shutil.move(str(target), str(backup))
            shutil.move(str(tmp_exe), str(target))
        finally:
            if temp_dir and temp_dir.exists():
                shutil.rmtree(temp_dir, ignore_errors=True)

    def _download_restic_release(self):
        # Скачивание с GitHub (urlopen timeout=120) — в фоне, UI не морозим.
        if getattr(self, "_restic_net_busy", False):
            return
        self._restic_net_busy = True
        target = self._download_restic_target()
        self._set_restic_status("⏳ Проверяю и скачиваю новую версию restic…", "#555")
        QApplication.setOverrideCursor(Qt.WaitCursor)
        def _do():
            try:
                self._download_restic_to(target)
                version = self._get_restic_version(str(target)) or "неизвестно"
                err = None
            except Exception as e:
                version, err = "", str(e)
            def _ui():
                self._restic_net_busy = False
                QApplication.restoreOverrideCursor()
                if err is None:
                    if hasattr(self, "edit_restic"):
                        self.edit_restic.setText(str(target))
                    self.cfg.restic = str(target)
                    self._set_restic_status(f"✅ Загружен restic {version} в {target}", "#006600")
                    self._changed()
                    QMessageBox.information(self, "restic", f"restic {version} загружен в:\n{target}")
                else:
                    msg = f"Не удалось загрузить restic:\n{err}"
                    self._set_restic_status(f"❌ {msg}", "#cc0000")
                    QMessageBox.warning(self, "restic", msg)
            self._ui_call_signal.emit(_ui)
        threading.Thread(target=_do, daemon=True).start()

    def _set_language(self, idx: int):
        code = self.cmb_language.itemData(idx)
        if code and code != self.cfg.gg("language", ""):
            self.cfg.sg("language", code)
            # Both languages at once: the message must be readable in either one.
            QMessageBox.information(self, "RestLex",
                "Язык изменится после перезапуска RestLex.\n"
                "Restart RestLex to apply the new language.")

    def _refresh_global_widgets(self):
        """Перечитать глобальные поля из cfg в виджеты (после импорта конфига)."""
        c = self.cfg
        to = c.global_backup_timeout_hours
        vals = {
            "edit_restic": c.gg("restic", ""), "edit_temp": c.temp,
            "ed_tgt": c.tg_token, "ed_tgc": c.tg_chat, "ed_tg_group": c.tg_group,
            "ed_report_url": c.report_url, "ed_agent_update_url": c.agent_auto_update_url,
            "ed_update_url": c.update_server_url, "ed_update_key": c.update_api_key,
            "ed_api_token": c.gg("api_token", ""),
            "chk_global_no_scan": c.global_no_scan, "chk_global_post_check": c.global_post_backup_check,
            "chk_check_host_only": c.global_check_host_only, "chk_cache_cleanup": c.cache_cleanup,
            "chk_auto_update": c.auto_update,
            "chk_tf": c.tg_fatal, "chk_te": c.tg_file, "chk_tp": c.tg_prune,
            "chk_tg_timeout": to > 0, "spin_global_bk_timeout": to if to > 0 else 4,
            "sp_vss_timeout": c.global_vss_timeout_s,
            "edit_global_pre_cmd": c.global_pre_backup_cmd,
        }
        for attr, v in vals.items():
            w = getattr(self, attr, None)
            if w is None:
                continue
            # blockSignals: toggled/valueChanged подключены к _save_global — иначе
            # он сработает посреди обновления и запишет полуобновлённые поля.
            w.blockSignals(True)
            try:
                if hasattr(w, "setChecked"):     w.setChecked(bool(v))
                elif hasattr(w, "setValue"):     w.setValue(int(v or 0))
                elif hasattr(w, "setPlainText"): w.setPlainText(str(v or ""))
                else:                            w.setText(str(v or ""))
            finally:
                w.blockSignals(False)
        self._refresh_restic_status_label()

    def _save_global(self) -> bool:
        """True — все настройки реально легли на диск. sg() теперь может отказать
        (конфиг залочен агентом / DPAPI недоступен), а раньше _saved() всё равно
        рисовал «✅ сохранено»; вдобавок _config_warn_busy схлопывает десяток
        предупреждений в одно окно, так что честный итог виден только по счётчику."""
        fails0 = self.cfg.write_failures
        self.cfg.restic=self.edit_restic.text().strip()
        self.cfg.temp=self.edit_temp.text().strip()
        if hasattr(self, "ed_tgt"):
            self.cfg.tg_token=self.ed_tgt.text().strip()
        if hasattr(self, "ed_tgc"):
            self.cfg.tg_chat=self.ed_tgc.text().strip()
        if hasattr(self, "ed_tg_group"):
            self.cfg.tg_group=self.ed_tg_group.text().strip()
        if hasattr(self, "ed_report_url"):
            self.cfg.report_url=self.ed_report_url.text().strip()
        if hasattr(self, "ed_agent_update_url"):
            self.cfg.agent_auto_update_url=self.ed_agent_update_url.text().strip()
        if hasattr(self, "chk_global_no_scan"):
            self.cfg.global_no_scan=self.chk_global_no_scan.isChecked()
        if hasattr(self, "chk_global_post_check"):
            self.cfg.global_post_backup_check=self.chk_global_post_check.isChecked()
            self.cfg.global_check_host_only=self.chk_check_host_only.isChecked()
            self.cfg.global_vss_timeout_s=self.sp_vss_timeout.value()
        if hasattr(self, "edit_global_pre_cmd"):
            self.cfg.global_pre_backup_cmd = self.edit_global_pre_cmd.toPlainText().strip()
        if hasattr(self, "spin_global_bk_timeout"):
            self.cfg.global_backup_timeout_hours = (
                self.spin_global_bk_timeout.value()
                if not hasattr(self, "chk_tg_timeout") or self.chk_tg_timeout.isChecked()
                else 0
            )
        if hasattr(self, "chk_cache_cleanup"):
            self.cfg.cache_cleanup = self.chk_cache_cleanup.isChecked()
        return self.cfg.write_failures == fails0

    def _save_settings(self):
        self._autosave(include_repo=False)
        ok = self._save_global()
        self._refresh_restic_status_label()
        if ok:
            self._saved()
        else:
            n = self.cfg.write_failures
            self.statusBar().showMessage(
                f"⚠ Часть настроек НЕ сохранена (отказов записи: {n}) — "
                f"конфиг занят или недоступен, повторите", 10000)

    def _on_tab(self, idx):
        if self._closing:
            return
        new_widget = self.tabs.widget(idx)
        old_widget = self._current_tab_widget
        if self._tab_change_guard:
            self._current_tab_widget = new_widget
            return
        if old_widget is getattr(self, "_tab_repo", None) and new_widget is not old_widget and self._repo_dirty:
            if not self._confirm_discard_repo_changes("Есть несохранённые изменения репозитория.\nОтменить их и перейти на другую вкладку?"):
                self._tab_change_guard = True
                self.tabs.setCurrentWidget(old_widget)
                self._tab_change_guard = False
                return
        self._autosave(include_repo=not self._repo_dirty)
        # Auto-save global/web settings when leaving their tabs
        if old_widget is not None and old_widget is not new_widget:
            if old_widget is getattr(self, "_tab_settings", None):
                self._save_global()
                self._refresh_restic_status_label()
            if old_widget is getattr(self, "_tab_web", None):
                self._wsvr_save_config()
                self._upd_save_config()
        self._current_tab_widget = new_widget
        if new_widget is getattr(self, "_tab_restore", None):
            QTimer.singleShot(0, self._activate_restore_tab)
        if new_widget is getattr(self, "_tab_backup", None) and self._logs_dirty:
            QTimer.singleShot(0, self._ref_logs)
        if new_widget is getattr(self, "_tab_logs", None):
            QTimer.singleShot(0, self._ref_logs)
        if new_widget is getattr(self, "_tab_scheduler", None):
            self._sched_refresh_status()
    def _changed(self): pass
    def _saved(self): self.statusBar().showMessage("✅ Настройки сохранены",4000)

    # ─── REPO TYPE ────────────────────────────────────────────────────────────

    def _rtype(self) -> str:
        r=self.rtype_list.currentRow()
        return REPO_TYPES[r][0] if 0<=r<len(REPO_TYPES) else "local"

    def _on_rtype(self,idx): self.rstack.setCurrentIndex(idx); self._val_addr()

    def _upd_rest_url(self):
        pg=self._rpages.get("rest",{})
        host=pg.get("rest_host",QLineEdit()).text().strip()
        path=pg.get("rest_path",QLineEdit()).text().strip()
        user=pg.get("rest_user",QLineEdit()).text().strip()
        pw=pg.get("rest_pass",QLineEdit()).text().strip()
        sch=pg.get("rest_scheme",QLineEdit()).text().strip() or "http"
        if not path.startswith("/"): path="/"+path
        # спецсимволы в логине/пароле (@ : / % #) ломают URL — _parse_rest_url ждёт percent-encoding
        qu=urllib.parse.quote(user,safe=""); qp=urllib.parse.quote(pw,safe="")
        if user and pw: url=f"rest:{sch}://{qu}:{qp}@{host}{path}"
        elif user:      url=f"rest:{sch}://{qu}@{host}{path}"
        else:           url=f"rest:{sch}://{host}{path}"
        u=pg.get("rest_url")
        if u: u.setText(url)
        v=pg.get("val")
        if v:
            if host: v.setText("")
            else: v.setText("⚠ Укажите хост:порт")

    def _parse_rest_url(self, refs: dict):
        url = refs.get("rest_url", QLineEdit()).text().strip()
        if url.startswith("rest:"): url = url[5:]
        try:
            p = urllib.parse.urlparse(url)
            host_port = p.hostname or ""
            if p.port: host_port = f"{host_port}:{p.port}"
            path = p.path or "/"
            user = urllib.parse.unquote(p.username or "")
            pw = urllib.parse.unquote(p.password or "")
            sch = (p.scheme or "http").lower()
            for key, val in [("rest_host", host_port), ("rest_path", path), ("rest_user", user), ("rest_pass", pw), ("rest_scheme", sch)]:
                w = refs.get(key)
                if w: w.setText(val)
        except Exception:
            pass

    def _val_addr(self):
        rt=self._rtype()
        if rt=="rest": self._upd_rest_url(); return
        pg=self._rpages.get(rt,{})
        addr=pg.get("addr",QLineEdit()).text().strip()
        v=pg.get("val")
        if not v: return
        if not addr: v.setText(""); pg.get("addr",QLineEdit()).setStyleSheet(""); return
        pat=[r[5] for r in REPO_TYPES if r[0]==rt][0]
        if pat and not re.match(pat,addr):
            ex=[r[4] for r in REPO_TYPES if r[0]==rt][0]
            v.setText(f"⚠ Неверный формат. Пример: {ex}")
            pg.get("addr",QLineEdit()).setStyleSheet("border:2px solid #cc0000;background:#fff0f0;")
        else:
            v.setText("✓ Формат верный"); v.setStyleSheet("color:#006600;")
            pg.get("addr",QLineEdit()).setStyleSheet("")

    def _get_repo_path(self) -> str:
        rt=self._rtype()
        if rt=="rest": return self._rpages["rest"].get("rest_url",QLineEdit()).text().strip()
        return self._rpages.get(rt,{}).get("addr",QLineEdit()).text().strip()

    def _load_pw_file(self):
        path,_=QFileDialog.getOpenFileName(self,"Файл пароля")
        if not path: return
        try:
            self.edit_pw.setText(Path(path).read_text(encoding="utf-8").strip())
            self._repo_changed()
        except Exception as e:
            QMessageBox.warning(self,"Ошибка",f"Не удалось прочитать:\n{e}")

    def _validate_req(self) -> Optional[str]:
        rt=self._rtype(); pg=self._rpages.get(rt,{}); errs=[]
        if rt=="rest":
            h=pg.get("rest_host",QLineEdit()).text().strip()
            if not h:
                w=pg.get("rest_host")
                if w: w.setStyleSheet("border:2px solid #cc0000;background:#fff0f0;")
                errs.append("• Хост:порт обязателен")
            else:
                w=pg.get("rest_host")
                if w: w.setStyleSheet("")
        else:
            a=pg.get("addr",QLineEdit()).text().strip()
            if not a:
                w=pg.get("addr")
                if w: w.setStyleSheet("border:2px solid #cc0000;background:#fff0f0;")
                ex=[r[4] for r in REPO_TYPES if r[0]==rt][0]
                errs.append(f"• Адрес репозитория обязателен. Пример: {ex}")
            else:
                w=pg.get("addr")
                if w: w.setStyleSheet("")
        if not self.edit_pw.text().strip():
            self.edit_pw.setStyleSheet("border:2px solid #cc0000;background:#fff0f0;")
            errs.append("• Пароль обязателен")
        else:
            self.edit_pw.setStyleSheet("")
        return "\n".join(errs) if errs else None

    def _save_check(self):
        if not self.pid:
            self.lbl_chk.setText("❌ Выберите профиль"); return
        err=self._validate_req()
        if err:
            self.lbl_chk.setText("⛔ Заполните поля:\n"+err)
            self.lbl_chk.setStyleSheet("color:#cc0000;"); return
        # "Сохранить и проверить" должна РЕАЛЬНО сохранить введённый пароль,
        # иначе он только проверяется, но не пишется на диск, а вкладка остаётся
        # "грязной" (_repo_dirty=True) → ложное "есть несохранённые изменения".
        if not self._save_repo():
            return
        self.lbl_chk.setText("⏳ Проверяю…"); self.lbl_chk.setStyleSheet("color:#555;")
        self.btn_chk.setEnabled(False)
        self._chk_errs=[]
        prof = dict(self.cfg.profile(self.pid) or {})
        for key in self._all_repo_field_keys():
            prof.pop(key, None)
        prof.update(self._repo_data_from_ui())
        w=self._mk_worker_for_profile(prof)
        if not w: self.btn_chk.setEnabled(True); return
        self._chk_worker=w
        w.err.connect(lambda m: self._chk_errs.append(m))
        w.out.connect(lambda m: self._log(m))
        w.done.connect(self._on_chk_done)
        self._chk_timer=QTimer(self); self._chk_timer.setSingleShot(True)
        self._chk_timer.timeout.connect(self._on_chk_timeout)
        self._chk_timer.start(15000); w.cat_cfg()

    def _on_chk_timeout(self):
        if self._chk_worker:
            try: self._chk_worker.proc.kill()
            except: pass
        self.btn_chk.setEnabled(True)
        self.lbl_chk.setText("❌ Таймаут (15с) — нет ответа")
        self.lbl_chk.setStyleSheet("color:#cc0000;")

    def _on_chk_done(self,code,out):
        if self.sender() is not self._chk_worker:
            return
        if self._chk_timer and self._chk_timer.isActive(): self._chk_timer.stop()
        self.btn_chk.setEnabled(True)
        if code==0:
            self.lbl_chk.setText("✅ Репозиторий доступен")
            self.lbl_chk.setStyleSheet("color:#006600;")
        else:
            detail=self._chk_errs[0].strip() if self._chk_errs else ""
            is_auth_error = any(x in detail.lower() for x in ("unauthorized","forbidden","401","403","invalid credentials"))
            if ("does not exist" in detail or "unable to open config" in detail or code==1) and not is_auth_error:
                # Не показываем промежуточный "не найден" ДО диалога — иначе
                # пользователь видит вспышку ошибки, а через секунду "доступен",
                # хотя по факту это штатный сценарий первого запуска.
                # "Проверяю…" остаётся на экране, пока не будет финального вердикта.
                if QMessageBox.question(self,"Репозиторий не найден",
                    "Репозиторий не существует. Создать новый?"
                ) == QMessageBox.Yes:
                    self._init_repo(on_success=self._save_check)
                else:
                    self.lbl_chk.setText("⚠️ Репозиторий не существует"); self.lbl_chk.setStyleSheet("color:#885500;")
            else:
                if is_auth_error:
                    msg="🔑 Ошибка авторизации — проверьте логин и пароль"+(("\n   "+detail) if detail else "")
                else:
                    msg="❌ Нет доступа"+(("\n   "+detail) if detail else "")
                self.lbl_chk.setText(msg); self.lbl_chk.setStyleSheet("color:#cc0000;")

    # ─── WORKER FACTORY ──────────────────────────────────────────────────────

    def _mk_worker(self) -> Optional[ResticWorker]:
        if not self.pid: return None
        prof=self.cfg.profile(self.pid) or {}
        return self._mk_worker_for_profile(prof)

    def _mk_worker_for_profile(self, prof: Optional[dict]) -> Optional[ResticWorker]:
        prof = prof or {}
        repo=prof.get("repo_path",""); pw=prof.get("repo_password","")
        rt=prof.get("repo_type","local")
        if not repo: self._log("❌ Репозиторий не задан","ERROR"); return None
        if not pw:   self._log("❌ Пароль не задан","ERROR"); return None
        extra={}
        if rt=="s3":   extra={"AWS_ACCESS_KEY_ID":prof.get("s3_k",""),"AWS_SECRET_ACCESS_KEY":prof.get("s3_s","")}
        elif rt=="b2": extra={"B2_ACCOUNT_ID":prof.get("b2_i",""),"B2_ACCOUNT_KEY":prof.get("b2_k","")}
        elif rt=="azure": extra={"AZURE_ACCOUNT_NAME":prof.get("az_a",""),"AZURE_ACCOUNT_KEY":prof.get("az_k","")}
        elif rt=="gs": extra={"GOOGLE_PROJECT_ID":prof.get("gs_p",""),"GOOGLE_APPLICATION_CREDENTIALS":prof.get("gs_j","")}
        return ResticWorker(self.cfg.restic,repo,pw,self.cfg.temp,extra)

    # ─── SNAPSHOTS ───────────────────────────────────────────────────────────

    def _active_snap_cache_key(self) -> str:
        prof = self.cfg.profile(self.pid) or {}
        payload = "|".join([
            self.pid or "",
            prof.get("repo_type", ""),
            prof.get("repo_path", ""),
        ])
        return hashlib.sha1(payload.encode("utf-8")).hexdigest()[:16]

    def _snap_cache_dir(self) -> Path:
        d = self.cfg.BASE / "restore_cache" / self._active_snap_cache_key()
        d.mkdir(parents=True, exist_ok=True)
        return d

    def _snapshots_cache_file(self) -> Optional[Path]:
        if not self.pid:
            return None
        return self._snap_cache_dir() / "snapshots.json"

    def _snap_nodes_cache_file(self, sid: str) -> Optional[Path]:
        if not self.pid or not sid:
            return None
        return self._snap_cache_dir() / f"{sid}.json"

    def _parse_snap_time(self, value: str) -> Optional[datetime]:
        raw = (value or "").strip()
        if not raw:
            return None
        try:
            return datetime.fromisoformat(raw.replace("Z", "+00:00"))
        except ValueError:
            return None

    def _get_snap_explicit_type(self, snap: dict) -> str:
        tag_map = {
            "monthly": "МЕСЯЧНЫЙ",
            "month": "МЕСЯЧНЫЙ",
            "weekly": "НЕДЕЛЬНЫЙ",
            "week": "НЕДЕЛЬНЫЙ",
            "daily": "ДНЕВНОЙ",
            "day": "ДНЕВНОЙ",
        }
        found = {tag_map.get(str(tag).strip().lower()) for tag in (snap.get("tags") or [])}
        for label in ("МЕСЯЧНЫЙ", "НЕДЕЛЬНЫЙ", "ДНЕВНОЙ"):
            if label in found:
                return label
        return ""

    def _mark_snapshot_types(self, snaps: List[dict]):
        daily_seen = set()
        weekly_seen = set()
        monthly_seen = set()
        for snap in snaps:
            label = self._get_snap_explicit_type(snap)
            dt = self._parse_snap_time(snap.get("time", ""))
            if dt:
                day_key = dt.date().isoformat()
                iso_week = dt.isocalendar()
                week_key = (iso_week.year, iso_week.week)
                month_key = (dt.year, dt.month)
                if not label:
                    if month_key not in monthly_seen:
                        label = "МЕСЯЧНЫЙ"
                    elif week_key not in weekly_seen:
                        label = "НЕДЕЛЬНЫЙ"
                    else:
                        label = "ДНЕВНОЙ"
                daily_seen.add(day_key)
                weekly_seen.add(week_key)
                monthly_seen.add(month_key)
            elif not label:
                label = "ДНЕВНОЙ"
            snap["_type_label"] = label

    def _format_snap_label(self, snap: dict, idx: int = 0) -> str:
        dt = self._parse_snap_time(snap.get("time", ""))
        n = idx + 1
        if dt:
            return f"Копия {n}  /  {dt.strftime('%H:%M')}  /  {dt.strftime('%d.%m.%Y')}"
        raw = ((snap.get("time") or "").strip() or "неизвестно")
        return f"Копия {n}  /  {raw}"

    def _apply_snapshots(self, snaps: List[dict], mark_fresh=False, auto_select=True):
        self._snaps = snaps
        if mark_fresh:
            self._snap_source_pid = self.pid
        self.snap_combo.blockSignals(True)
        self.snap_combo.clear()
        for i, snap in enumerate(snaps):
            self.snap_combo.addItem(self._format_snap_label(snap, i), snap["id"])
        self.snap_combo.blockSignals(False)
        if snaps:
            self._update_restore_visibility(True)
            if auto_select:
                self._select_latest_snap()
            else:
                self._cur_snap_id = snaps[0].get("id")
                self.snap_load_lbl.setText("← Откройте вкладку «Восстановление», чтобы загрузить содержимое снапшота")
                self.snap_tree.clear()
                self.snap_path_lbl.clear()
        else:
            self._update_restore_visibility(False, message="Бэкапов пока нет.\nСначала запустите бэкап для этого профиля.")
            self.snap_load_lbl.setText("Снапшоты не найдены")
            self.snap_tree.clear()
            self.snap_path_lbl.clear()

    def _save_snapshots_cache(self, snaps: List[dict]):
        fp = self._snapshots_cache_file()
        if not fp:
            return
        try:
            fp.write_text(
                json.dumps({"snaps": snaps}, ensure_ascii=False, separators=(",", ":")),
                encoding="utf-8",
            )
        except Exception as e:
            self._log(f"⚠ Не удалось сохранить кеш снапшотов: {e}", "ERROR")

    def _load_snapshots_cache(self) -> bool:
        fp = self._snapshots_cache_file()
        if not fp or not fp.exists():
            return False
        try:
            payload = json.loads(fp.read_text(encoding="utf-8"))
            snaps = payload.get("snaps", payload) if isinstance(payload, dict) else payload
            if not isinstance(snaps, list):
                return False
        except Exception as e:
            self._log(f"⚠ Не удалось прочитать кеш снапшотов: {e}", "ERROR")
            return False
        snaps_sorted = sorted(snaps, key=lambda s: s.get("time", ""), reverse=True)
        self._mark_snapshot_types(snaps_sorted)
        self._apply_snapshots(snaps_sorted, mark_fresh=False, auto_select=self._is_restore_tab_active())
        return bool(snaps_sorted)

    def _load_snap_nodes_cache(self, sid: str) -> Optional[List[dict]]:
        fp = self._snap_nodes_cache_file(sid)
        if not fp or not fp.exists():
            return None
        try:
            payload = json.loads(fp.read_text(encoding="utf-8"))
            nodes = payload.get("nodes", payload) if isinstance(payload, dict) else payload
            if not isinstance(nodes, list):
                return None
        except Exception as e:
            self._log(f"⚠ Не удалось прочитать кеш путей снапшота: {e}", "ERROR")
            return None
        return nodes

    def _save_snap_nodes_cache(self, sid: str, nodes: List[dict]):
        fp = self._snap_nodes_cache_file(sid)
        if not fp:
            return
        try:
            fp.write_text(
                json.dumps({"nodes": nodes}, ensure_ascii=False, separators=(",", ":")),
                encoding="utf-8",
            )
        except Exception as e:
            self._log(f"⚠ Не удалось сохранить кеш путей снапшота: {e}", "ERROR")

    def _drop_snap_cache(self, sid: str):
        self._snap_nodes_cache.pop(sid, None)
        fp = self._snap_nodes_cache_file(sid)
        if fp and fp.exists():
            try:
                fp.unlink()
            except OSError:
                pass

    def _select_latest_snap(self):
        if self.snap_combo.count() <= 0:
            return
        sid = self.snap_combo.itemData(0)
        if not sid:
            return
        if self.snap_combo.currentIndex() != 0:
            self.snap_combo.setCurrentIndex(0)
        elif self._cur_snap_id != sid:
            self._on_snap_combo(0)

    def _activate_restore_tab(self):
        try:
            self._activate_restore_tab_impl()
        except Exception as e:
            self._log(f"⚠ Ошибка при открытии вкладки восстановления: {e}", "ERROR")

    def _activate_restore_tab_impl(self):
        if self._closing:
            return
        if not self.pid:
            self.snap_overlay.hide()
            self._update_restore_visibility(False, message="Выберите профиль.")
            return
        if self._del_mode:
            # Идёт сравнение с диском: вкладку не трогаем, прогресс остаётся на месте.
            return
        if self._snap_loading:
            self._update_restore_visibility(False, loading=True)
            return
        prof = self.cfg.profile(self.pid) or {}
        if prof.get("repo_path") and prof.get("repo_password"):
            if self._snaps and self._snap_source_pid == self.pid:
                self._update_restore_visibility(True)
                # Дерево пустое (снимок подгрузился, пока вкладка была закрыта) — наполняем;
                # иначе оставляем всё как было, чтобы не сбрасывать выбранный снимок и папку.
                if self.snap_tree.topLevelItemCount() == 0 and not self._ls_worker \
                        and self._snap_view_mode != "search":
                    self._select_latest_snap()
                return
            self._refresh_snaps(show_loading=True)
            return
        if self._snaps:
            self._update_restore_visibility(True)
            self._select_latest_snap()
            return
        self.snap_overlay.hide()
        self._update_restore_visibility(False, message="Бэкапов пока нет.\nСначала запустите бэкап для этого профиля.")

    def _reset_snaps(self, loading: bool = False):
        if hasattr(self, "btn_show_deleted"):
            self._cancel_deleted_scan(); self._del_mode = None; self._deleted_busy(False)
            self._clear_deleted_tree()
        self.snap_combo.blockSignals(True); self.snap_combo.clear()
        self.snap_combo.blockSignals(False)
        self._snaps=[]; self._cur_snap_id=None; self._snap_loading=False; self._snap_refresh_pending=False; self._snap_source_pid=None
        self._snap_browser_unc_target=""; self._snap_browser_last_target=""
        self._close_snap_browser()
        self._snap_nodes_cache.clear()
        self.snap_tree.clear()
        self.snap_path_lbl.clear()
        self.snap_load_lbl.setText("← Выберите снапшот для просмотра содержимого")
        if loading:
            self._update_restore_visibility(False, loading=True)
        else:
            self._update_restore_visibility(False, message="Бэкапов пока нет.\nСначала запустите бэкап для этого профиля.")

    def _refresh_snaps(self, show_loading=True):
        if self._closing:
            self._hide_loading()
            return
        if not self.pid: return
        if self._snap_loading:
            self._snap_refresh_pending = True
            self._log("snapshots refresh queued", channel="debug")
            return
        if show_loading:
            self._reset_snaps(loading=True)
            self.snap_combo.addItem("⏳ Загружаю снапшоты…")
            self._update_restore_visibility(False, loading=True)
        else:
            self.statusBar().showMessage("⏳ Обновляю список снапшотов…", 3000)
        self._log(f"snapshots refresh start: profile={self.pid}", channel="debug")
        if self._snapshots_worker is not None:
            self._stop_worker(self._snapshots_worker)
            self._snapshots_worker = None
        self.btn_snap_ref.setEnabled(False); self._snap_raw=[]; self._snap_loading=True; self._snap_refresh_pending=False
        w=self._mk_worker()
        if not w:
            self.snap_overlay.hide()
            self._hide_loading()
            self.btn_snap_ref.setEnabled(True); self._snap_loading=False; return
        self._snapshots_worker=w
        w.out.connect(lambda m:self._snap_raw.append(m))
        w.err.connect(lambda m:self._log("❌ "+m,"ERROR"))
        w.done.connect(self._on_snaps_done); w.snapshots()

    def _on_snaps_done(self,code,_):
        if self.sender() is not self._snapshots_worker:
            return
        if self._closing:
            self._snap_loading=False
            return
        self._snapshots_worker = None
        self._snap_loading=False
        self.btn_snap_ref.setEnabled(True)
        self._log(f"snapshots refresh done: code={code}", channel="debug")
        if code!=0:
            self.snap_overlay.hide()
            self._update_restore_visibility(False, message="Не удалось получить список снапшотов.")
            self._log("Ошибка получения снапшотов","ERROR")
            if self._snap_refresh_pending and not self._closing:
                self._snap_refresh_pending = False
                QTimer.singleShot(0, lambda: self._refresh_snaps(show_loading=self._is_restore_tab_active()))
            return
        try:
            raw=" ".join(self._snap_raw)
            snaps=json.loads(raw)
        except Exception as e:
            self.snap_overlay.hide()
            self._update_restore_visibility(False, message="Не удалось прочитать список снапшотов.")
            self._log(f"❌ Ошибка разбора снапшотов: {e}","ERROR")
            if self._snap_refresh_pending and not self._closing:
                self._snap_refresh_pending = False
                QTimer.singleShot(0, lambda: self._refresh_snaps(show_loading=self._is_restore_tab_active()))
            return
        # Sort: newest first
        snaps_sorted=sorted(snaps,key=lambda s:s.get("time",""),reverse=True)
        self._mark_snapshot_types(snaps_sorted)
        self._save_snapshots_cache(snaps_sorted)
        if not snaps_sorted:
            self.snap_overlay.hide()
        self._log(f"snapshots loaded: {len(snaps_sorted)}", channel="debug")
        self._apply_snapshots(
            snaps_sorted,
            mark_fresh=True,
            auto_select=self._is_restore_tab_active(),
        )
        if self._snap_refresh_pending and not self._closing:
            self._snap_refresh_pending = False
            QTimer.singleShot(0, lambda: self._refresh_snaps(show_loading=self._is_restore_tab_active()))

    def _on_snap_combo(self,idx):
        if idx<0 or idx>=self.snap_combo.count(): return
        sid=self.snap_combo.itemData(idx)
        if not sid: return
        self._cur_snap_id=sid
        if self._reset_folder_deleted():
            self._set_snap_view_mode("tree")
            self._set_del_status("")
        if hasattr(self, "snap_search_edit") and self.snap_search_edit.text():
            self.snap_search_edit.blockSignals(True)
            self.snap_search_edit.clear()
            self.snap_search_edit.blockSignals(False)
            self.btn_search_clear.setVisible(False)
            self.lbl_search_status.setText("")
            self._cancel_snap_search()
            self._set_snap_view_mode("tree")
        # Update header
        if idx<len(self._snaps):
            s=self._snaps[idx]
            hdr = f"Снапшот: {self._format_snap_label(s)}"
            paths = ", ".join(s.get("paths", []))
            if paths:
                hdr += f"  |  {paths}"
            self.snap_load_lbl.setText(hdr)
        self._load_snap_files(sid)

    def _using_snap_browser(self) -> bool:
        return (
            not self._restore_in_progress
            and bool(getattr(self, "snap_browser_host", None))
            and self._snap_view_mode == "browser"
        )

    def _set_snap_view_mode(self, mode: str):
        if mode == "browser" and not getattr(self, "snap_browser_host", None):
            mode = "tree"
        self._snap_view_mode = mode
        if hasattr(self, "snap_view_stack"):
            has_browser = bool(getattr(self, "snap_browser_host", None))
            tree_idx = 1 if has_browser else 0
            search_idx = 2 if has_browser else 1
            if mode == "search":
                idx = search_idx
            elif mode == "browser" and has_browser:
                idx = 0
            else:
                idx = tree_idx
            self.snap_view_stack.setCurrentIndex(idx)
        for btn, checked in [
            (getattr(self, "btn_snap_browser", None), mode == "browser"),
            (getattr(self, "btn_snap_tree", None), mode == "tree"),
        ]:
            if btn:
                btn.blockSignals(True)
                btn.setChecked(checked)
                btn.blockSignals(False)
        self._sync_snap_path_label()

    def _snap_browser_tick(self):
        self._snap_browser_resize()
        self._sync_snap_path_label()

    def _snap_browser_hint_text(self, text: str = ""):
        lbl = getattr(self, "snap_browser_hint", None)
        if not lbl:
            return
        if text:
            lbl.setText(text)
            lbl.show()
        else:
            lbl.hide()

    def _normalize_win_path(self, path: str) -> str:
        raw = (path or "").strip().strip('"')
        if not raw:
            return ""
        raw = raw.replace("/", "\\")
        if raw.startswith("\\\\"):
            return "\\\\" + re.sub(r"\\+", r"\\", raw[2:]).rstrip("\\/")
        return re.sub(r"\\+", r"\\", raw).rstrip("\\/")

    def _file_url_to_win_path(self, url: str) -> str:
        raw = urllib.parse.unquote((url or "").strip())
        low = raw.lower()
        if low.startswith("file:///"):
            path = raw[8:].replace("/", "\\")
            if re.match(r"^[a-z]:", path, re.I):
                return self._normalize_win_path(path)
            return self._normalize_win_path("\\\\" + path.lstrip("\\"))
        if low.startswith("file://"):
            return self._normalize_win_path("\\\\" + raw[7:].replace("/", "\\").lstrip("\\"))
        return ""

    def _shell_dispatch(self):
        if not _HAS_SHELL_EMBED:
            return None
        try:
            pythoncom.CoInitialize()
        except Exception:
            pass
        try:
            if self._shell_app is None:
                self._shell_app = win32com.client.Dispatch("Shell.Application")
            return self._shell_app
        except Exception:
            self._shell_app = None
            return None

    def _shell_windows(self) -> List[object]:
        shell = self._shell_dispatch()
        if not shell:
            return []
        try:
            windows = shell.Windows()
        except Exception:
            self._shell_app = None
            return []
        try:
            count = int(windows.Count)
        except Exception:
            try:
                return list(windows)
            except Exception:
                return []
        out = []
        for i in range(count):
            try:
                out.append(windows.Item(i))
            except Exception:
                continue
        return out

    def _snap_browser_window(self, hwnd: int = 0):
        target_hwnd = int(hwnd or getattr(self, "_snap_browser_hwnd", 0) or 0)
        if not target_hwnd:
            return None
        for window in self._shell_windows():
            try:
                if int(window.HWND) == target_hwnd:
                    return window
            except Exception:
                continue
        return None

    def _shell_window_path(self, window) -> str:
        if not window:
            return ""
        try:
            doc = window.Document
            folder = getattr(doc, "Folder", None)
            self_obj = getattr(folder, "Self", None) if folder else None
            path = getattr(self_obj, "Path", "") if self_obj else ""
            if path:
                return self._normalize_win_path(str(path))
        except Exception:
            pass
        try:
            return self._file_url_to_win_path(str(getattr(window, "LocationURL", "") or ""))
        except Exception:
            return ""

    def _enumerate_explorer_windows(self) -> List[int]:
        if not _HAS_SHELL_EMBED:
            return []
        out = []
        def _enum(hwnd, _):
            try:
                if not win32gui.IsWindowVisible(hwnd):
                    return True
                if win32gui.GetClassName(hwnd) not in ("CabinetWClass", "ExploreWClass"):
                    return True
                out.append(int(hwnd))
            except Exception:
                pass
            return True
        try:
            win32gui.EnumWindows(_enum, None)
        except Exception:
            return []
        return out

    def _wait_new_explorer_window(self, before: set, target: str, timeout: float = 10.0) -> int:
        deadline = time.time() + timeout
        norm_target = self._normalize_win_path(target).lower()
        while time.time() < deadline:
            QApplication.processEvents()
            fresh = [hwnd for hwnd in self._enumerate_explorer_windows() if hwnd not in before]
            if fresh:
                for hwnd in fresh:
                    path = self._shell_window_path(self._snap_browser_window(hwnd)).lower()
                    if path and (path == norm_target or path.startswith(norm_target + "\\")):
                        return hwnd
                return fresh[-1]
            time.sleep(0.2)
        return 0

    def _close_snap_browser(self):
        hwnd = int(getattr(self, "_snap_browser_hwnd", 0) or 0)
        style = self._snap_browser_style
        exstyle = self._snap_browser_exstyle
        self._snap_browser_hwnd = 0
        self._snap_browser_style = None
        self._snap_browser_exstyle = None
        self._snap_browser_hint_text("Explorer mount откроется здесь")
        if not hwnd or not _HAS_SHELL_EMBED:
            return
        try:
            if not win32gui.IsWindow(hwnd):
                return
            try:
                if style is not None:
                    win32gui.SetWindowLong(hwnd, win32con.GWL_STYLE, int(style))
                if exstyle is not None:
                    win32gui.SetWindowLong(hwnd, win32con.GWL_EXSTYLE, int(exstyle))
            except Exception:
                pass
            try:
                win32gui.SetParent(hwnd, 0)
            except Exception:
                pass
            try:
                win32gui.PostMessage(hwnd, win32con.WM_CLOSE, 0, 0)
            except Exception:
                pass
        except Exception:
            pass

    def _snap_browser_resize(self):
        hwnd = int(getattr(self, "_snap_browser_hwnd", 0) or 0)
        host = getattr(self, "snap_browser_host", None)
        if not hwnd or not host or not _HAS_SHELL_EMBED:
            return
        try:
            if not win32gui.IsWindow(hwnd):
                self._snap_browser_hwnd = 0
                self._snap_browser_last_target = ""
                self._snap_browser_hint_text("Explorer mount закрыт")
                return
            rect = host.rect()
            if rect.width() < 2 or rect.height() < 2:
                return
            win32gui.SetWindowPos(
                hwnd, 0, 0, 0, rect.width(), rect.height(),
                win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE | win32con.SWP_SHOWWINDOW | win32con.SWP_FRAMECHANGED
            )
        except Exception:
            pass

    def _attach_snap_browser(self, hwnd: int):
        host = getattr(self, "snap_browser_host", None)
        if not host or not hwnd or not _HAS_SHELL_EMBED:
            raise RuntimeError("встроенный Explorer недоступен")
        self._close_snap_browser()
        host_hwnd = int(host.winId())
        orig_style = win32gui.GetWindowLong(hwnd, win32con.GWL_STYLE)
        orig_exstyle = win32gui.GetWindowLong(hwnd, win32con.GWL_EXSTYLE)
        win32gui.SetParent(hwnd, host_hwnd)
        style = (orig_style & ~(
            win32con.WS_CAPTION | win32con.WS_THICKFRAME | win32con.WS_MINIMIZEBOX |
            win32con.WS_MAXIMIZEBOX | win32con.WS_SYSMENU | win32con.WS_POPUP
        )) | win32con.WS_CHILD | win32con.WS_VISIBLE
        exstyle = orig_exstyle & ~(
            win32con.WS_EX_APPWINDOW | win32con.WS_EX_WINDOWEDGE |
            win32con.WS_EX_CLIENTEDGE | win32con.WS_EX_DLGMODALFRAME
        )
        win32gui.SetWindowLong(hwnd, win32con.GWL_STYLE, style)
        win32gui.SetWindowLong(hwnd, win32con.GWL_EXSTYLE, exstyle)
        self._snap_browser_hwnd = int(hwnd)
        self._snap_browser_style = orig_style
        self._snap_browser_exstyle = orig_exstyle
        self._snap_browser_hint_text("")
        self._snap_browser_resize()

    def _mount_snapshot_dirname(self, snap: dict, idx: int) -> str:
        dt = self._parse_snap_time(snap.get("time", ""))
        if dt:
            try:
                dt = dt.astimezone()
            except Exception:
                pass
            stamp = dt.strftime("%Y-%m-%d_%H-%M-%S")
        else:
            stamp = "unknown"
        short_id = (snap.get("short_id") or snap.get("id") or "")[:8] or "snapshot"
        return f"{idx+1:04}__{stamp}__{short_id}"

    def _ensure_snap_browser_mount(self) -> str:
        if self._closing:
            raise RuntimeError("приложение закрывается")
        if self._restore_in_progress:
            raise RuntimeError("WebDAV отключён во время восстановления")
        self._ensure_agent_api()
        info = agent_api_request("/mount/status", timeout=5)
        if not isinstance(info, dict):
            info = {}
        if not info.get("active") or info.get("pid") != self.pid:
            data = agent_api_request(
                f"/mount/start/{urllib.parse.quote(self.pid or '')}",
                method="POST",
                timeout=20,
            )
            if not isinstance(data, dict) or not data.get("ok"):
                raise RuntimeError((data or {}).get("error", "mount не запустился"))
            info = data.get("mount") or {}
        self._mount_apply_status(info)
        unc = self._mount_unc_from_info(info)
        if not unc:
            raise RuntimeError("служба не вернула путь mount")
        self._snap_browser_unc_root = unc.rstrip("\\/")
        return self._snap_browser_unc_root

    def _snap_browser_target(self) -> str:
        if not self.pid or not self._cur_snap_id or not self._snaps:
            return ""
        try:
            idx = next(i for i, snap in enumerate(self._snaps) if snap.get("id") == self._cur_snap_id)
        except StopIteration:
            return ""
        unc_root = self._ensure_snap_browser_mount()
        dirname = self._mount_snapshot_dirname(self._snaps[idx], idx)
        return unc_root + "\\snapshots\\" + dirname

    def _open_snap_browser(self, target: str):
        if not _HAS_SHELL_EMBED:
            raise RuntimeError("встроенный Explorer недоступен")
        before = set(self._enumerate_explorer_windows())
        launched = False
        for args in (
            ["explorer.exe", "/separate,", target],
            ["explorer.exe", target],
        ):
            try:
                subprocess.Popen(args)
                launched = True
                break
            except Exception:
                continue
        if not launched:
            shell = self._shell_dispatch()
            if shell:
                for method in ("Explore", "Open"):
                    try:
                        getattr(shell, method)(target)
                        launched = True
                        break
                    except Exception:
                        continue
        hwnd = self._wait_new_explorer_window(before, target)
        if not hwnd:
            raise RuntimeError("Explorer не открыл отдельное окно")
        self._attach_snap_browser(hwnd)

    def _sync_snap_browser(self):
        if self._closing or not self._using_snap_browser() or not self.pid or not self._cur_snap_id:
            return
        try:
            target = self._snap_browser_target()
            if not target:
                return
            self._snap_browser_unc_target = target.rstrip("\\/")
            if (
                getattr(self, "_snap_browser_last_target", "") == self._snap_browser_unc_target
                and getattr(self, "_snap_browser_hwnd", 0)
            ):
                self._snap_browser_resize()
                return
            self._snap_browser_last_target = self._snap_browser_unc_target
            self._open_snap_browser(self._snap_browser_unc_target)
            self.snap_path_lbl.setText("/")
        except Exception as e:
            self._log(f"⚠ Встроенный Explorer: {e}", "ERROR")
            self._close_snap_browser()
            self._set_snap_view_mode("tree")

    def _browser_query_paths(self) -> List[str]:
        window = self._snap_browser_window()
        if not window:
            return []
        try:
            doc = window.Document
            items = doc.SelectedItems()
            count = int(items.Count)
            out = []
            for i in range(count):
                item = items.Item(i)
                path = str(getattr(item, "Path", "") or "")
                if path:
                    out.append(self._normalize_win_path(path))
            return out
        except Exception:
            return []

    def _browser_current_unc_path(self) -> str:
        path = self._shell_window_path(self._snap_browser_window())
        return path or self._snap_browser_unc_target

    def _browser_unc_to_snap_path(self, unc_path: str) -> str:
        raw = self._normalize_win_path(unc_path)
        base = self._normalize_win_path(self._snap_browser_unc_target)
        if not raw or not base:
            return ""
        if raw.lower() == base.lower():
            return "/"
        if not raw.lower().startswith(base.lower() + "\\"):
            return ""
        rel = raw[len(base):].lstrip("\\/")
        if not rel:
            return "/"
        return "/" + rel.replace("\\", "/")

    def _browser_selected_snap_paths(self) -> List[str]:
        return compact_snap_paths(
            [self._browser_unc_to_snap_path(path) for path in self._browser_query_paths() if path]
        )

    def _browser_current_snap_path(self) -> str:
        return self._browser_unc_to_snap_path(self._browser_current_unc_path())

    def closeEvent(self, event):
        self._shutdown_runtime()
        event.accept()
        super().closeEvent(event)

    def _load_snap_files(self,sid):
        if self._closing:
            self._hide_loading()
            return
        self.snap_tree.clear()
        self.snap_path_lbl.setText("/")
        self.snap_load_lbl.setText("⏳ Читаю корень снапшота…")
        self._load_snap_dir(sid, "/", parent_item=None, initial=True)

    def _format_snap_mtime(self, value: str) -> str:
        raw = (value or "").strip()
        if not raw:
            return ""
        raw = raw.replace("Z", "+00:00")
        try:
            dt = datetime.fromisoformat(raw)
            return dt.strftime("%d.%m.%Y %H:%M")
        except ValueError:
            return raw[:16].replace("T", " ")

    def _snap_child_nodes_from_lines(self, lines: List[str], parent_path: str) -> List[dict]:
        parent_path = normalize_snap_path(parent_path)
        nodes = []
        for ln in lines:
            try:
                obj=json.loads(ln)
            except Exception:
                if ln.strip():
                    self._log(f"⚠ Не удалось разобрать строку ls: {ln.strip()}", "ERROR")
                continue
            if obj.get("struct_type") != "node":
                continue
            path = normalize_snap_path(obj.get("path", ""))
            if path in ("", parent_path):
                continue
            pure = PurePosixPath(path)
            if normalize_snap_path(str(pure.parent)) != parent_path:
                continue
            node = dict(obj)
            node["path"] = path
            node["name"] = node.get("name") or pure.name or path.strip("/")
            nodes.append(node)
        return sorted(
            nodes,
            key=lambda n: (0 if n.get("type") == "dir" else 1, str(n.get("name") or "").lower()),
        )

    def _make_snap_tree_item(self, node: dict) -> QTreeWidgetItem:
        path = normalize_snap_path(node.get("path", ""))
        is_dir = node.get("type") == "dir"
        name = node.get("name") or PurePosixPath(path).name or path.strip("/")
        size = "" if is_dir else human_sz(int(node.get("size") or 0))
        mtime = self._format_snap_mtime(node.get("mtime", ""))
        title = f"{'📁' if is_dir else '📄'} {name}"
        item = QTreeWidgetItem([title, size, mtime])
        item.setData(0, Qt.UserRole, path)
        item.setData(0, Qt.UserRole + 1, "dir" if is_dir else "file")
        item.setData(0, Qt.UserRole + 2, False)  # children loaded
        item.setData(0, Qt.UserRole + 3, False)  # children loading
        item.setData(0, Qt.UserRole + 5, name)
        item.setToolTip(0, path)
        if is_dir:
            self._add_snap_placeholder(item)
        return item

    def _add_snap_placeholder(self, parent_item: QTreeWidgetItem):
        ph = QTreeWidgetItem([""])
        ph.setData(0, Qt.UserRole + 4, True)
        parent_item.addChild(ph)

    def _clear_snap_placeholder(self, parent_item: QTreeWidgetItem):
        while parent_item.childCount():
            child = parent_item.child(0)
            if child.data(0, Qt.UserRole + 4):
                parent_item.removeChild(child)
            else:
                break

    def _populate_snap_dir(self, sid: str, parent_path: str, parent_item: Optional[QTreeWidgetItem], lines: List[str], initial: bool = False):
        if self._closing or self._cur_snap_id != sid:
            return
        nodes = self._snap_child_nodes_from_lines(lines, parent_path)
        if parent_item is None:
            self.snap_tree.clear()
            for node in nodes:
                self.snap_tree.addTopLevelItem(self._make_snap_tree_item(node))
            self.snap_overlay.hide()
            self.snap_tree.setEnabled(True)
            self._set_restore_buttons_enabled(True)
            self.snap_load_lbl.setText("← Выберите файл или папку в снапшоте")
            self.snap_tree.clearSelection()
            self._sync_snap_path_label()
            return
        parent_item.takeChildren()
        for node in nodes:
            parent_item.addChild(self._make_snap_tree_item(node))
        parent_item.setData(0, Qt.UserRole + 2, True)
        parent_item.setData(0, Qt.UserRole + 3, False)
        if not nodes:
            parent_item.setChildIndicatorPolicy(QTreeWidgetItem.DontShowIndicatorWhenChildless)
        self.snap_tree.setEnabled(True)
        self._set_restore_buttons_enabled(True)
        self.statusBar().clearMessage()

    def _load_snap_dir(self, sid: str, path: str, parent_item: Optional[QTreeWidgetItem], initial: bool = False):
        if self._closing:
            self._hide_loading()
            return
        if self._ls_worker:
            return
        self._log(f"snapshot ls start: {sid[:8]} path={path}", channel="debug")
        if initial:
            self.snap_overlay.setText("⏳  Читаю содержимое снапшота…")
            self.snap_overlay.show()
        else:
            self.statusBar().showMessage(f"⏳ Загружаю {path}…", 0)
        self.snap_tree.setEnabled(False); self._set_restore_buttons_enabled(False)
        if parent_item is not None:
            parent_item.setData(0, Qt.UserRole + 3, True)
        lines=[]; w=self._mk_worker()
        if not w:
            self.snap_overlay.hide(); self.snap_tree.setEnabled(True); self._set_restore_buttons_enabled(True); self._hide_loading(); return
        self._ls_worker=w
        w.out.connect(lambda m,buf=lines:buf.append(m))
        w.err.connect(lambda m:self._log("❌ "+m,"ERROR"))
        w.done.connect(lambda _,__,snap_id=sid,target=path,parent=parent_item,buf=lines,first=initial:self._on_ls_dir_done(snap_id, target, parent, buf, first))
        w.ls(sid, path)

    def _on_snap_item_expanded(self, item: QTreeWidgetItem):
        if self._closing or not item:
            return
        if item.data(0, Qt.UserRole + 1) != "dir":
            return
        if item.data(0, Qt.UserRole + 2) or item.data(0, Qt.UserRole + 3):
            return
        self._load_snap_dir(self._cur_snap_id, item.data(0, Qt.UserRole), item, initial=False)

    def _sync_snap_path_label(self):
        if self._closing:
            return
        if self._using_snap_browser():
            cur = self._browser_current_snap_path()
            self.snap_path_lbl.setText(cur or "/")
            return
        item = self.snap_tree.currentItem()
        self.snap_path_lbl.setText(item.data(0, Qt.UserRole) if item else "/")

    def _on_ls_dir_done(self, sid, path, parent_item, lines, initial=False):
        if self.sender() is not self._ls_worker:
            return
        self._ls_worker = None
        if self._closing:
            return
        self._populate_snap_dir(sid, path, parent_item, lines, initial=initial)
        self._log(f"snapshot ls done: {sid[:8]} path={path}", channel="debug")
        self._hide_loading()

    def _del_snap(self):
        idx=self.snap_combo.currentIndex()
        if idx<0: return
        sid=self.snap_combo.itemData(idx)
        if not sid: return
        s=self._snaps[idx] if idx<len(self._snaps) else {}
        short=s.get("short_id","?")
        if QMessageBox.question(self,"Удалить снапшот",
            f"Удалить снапшот {short}?\nБудет выполнен prune — займёт время.") != QMessageBox.Yes: return
        self.snap_combo.setItemText(idx,f"⏳ {short} — удаляется…")
        self.btn_snap_del.setEnabled(False)
        w=self._mk_worker()
        if not w: return
        w.out.connect(lambda m:self._log(m))
        w.err.connect(lambda m:self._log("❌ "+m,"ERROR"))
        w.done.connect(lambda c,_,i=idx,sh=short:self._on_snap_del_done(c,i,sh))
        w.forget_s(sid); self._snap_workers[sid]=w

    def _on_snap_del_done(self,code,idx,short):
        sender = self.sender()
        if sender not in self._snap_workers.values():
            return
        self.btn_snap_del.setEnabled(True)
        if code==0:
            sid = self.snap_combo.itemData(idx)
            if sid: self._drop_snap_cache(sid)
            self._snap_source_pid=None
            self.snap_combo.removeItem(idx)
            if idx<len(self._snaps): self._snaps.pop(idx)
            self.snap_tree.clear()
        else:
            self.snap_combo.setItemText(idx,f"❌ {short} — ошибка")

    # ─── LOCAL BROWSER ───────────────────────────────────────────────────────

    def _init_local(self):
        self._rebuild_drive_buttons()
        self._fill_local(self._local_path)

    def _rebuild_drive_buttons(self):
        while self.drives_row.count():
            item=self.drives_row.takeAt(0)
            if item.widget(): item.widget().deleteLater()
        if sys.platform=="win32":
            import string
            for lt in string.ascii_uppercase:
                dr=Path(f"{lt}:\\")
                if dr.exists():
                    b=QPushButton(f"{lt}:"); b.setFixedSize(34,24)
                    b.setStyleSheet("font-size:10px;padding:0;")
                    b.setToolTip(f"Диск {lt}:\\")
                    b.clicked.connect(lambda _,d=dr: (setattr(self,"_local_path",d),
                                                       self._fill_local(d)))
                    self.drives_row.addWidget(b)
        self.drives_row.addStretch()

    def _local_nav(self):
        p=Path(self.local_edit.text())
        if p.is_dir(): self._local_path=p; self._fill_local(p)

    def _local_up(self):
        par=self._local_path.parent
        if par==self._local_path:
            self._rebuild_drive_buttons(); return
        self._local_path=par; self._fill_local(par)

    def _fill_local(self,path:Path):
        self.local_tree.clear(); self.local_edit.setText(str(path))
        self._local_path=path
        try:
            ents=sorted(path.iterdir(),key=lambda p:(not p.is_dir(),p.name.lower()))
            for e in ents:
                try:
                    st=e.stat()
                    if e.is_dir():
                        it=QTreeWidgetItem([f"📁 {e.name}","",
                            datetime.fromtimestamp(st.st_mtime).strftime("%d.%m.%Y %H:%M")])
                        it.setData(0,Qt.UserRole+1,"dir")
                    else:
                        it=QTreeWidgetItem([e.name,human_sz(st.st_size),
                            datetime.fromtimestamp(st.st_mtime).strftime("%d.%m.%Y %H:%M")])
                        it.setData(0,Qt.UserRole+1,"file")
                    it.setData(0,Qt.UserRole,str(e)); self.local_tree.addTopLevelItem(it)
                except (PermissionError,OSError): pass
        except (PermissionError,OSError) as e:
            self._log(f"Ошибка чтения: {e}","ERROR")

    def _local_dbl(self,item,_):
        p=Path(item.data(0,Qt.UserRole) or "")
        if p.is_dir(): self._local_path=p; self._fill_local(p)

    def _refresh_local(self):
        if self._local_path.exists(): self._fill_local(self._local_path)

    def _local_ctx(self,pos:QPoint):
        item=self.local_tree.itemAt(pos); menu=QMenu(self)
        if item:
            p=Path(item.data(0,Qt.UserRole) or "")
            a_open=menu.addAction("📂 Открыть")
            a_open.triggered.connect(lambda:self._local_dbl(item,None))
            menu.addSeparator()
            a_ren=menu.addAction("✏️ Переименовать  [F2]")
            a_ren.triggered.connect(lambda:self._local_rename())
            a_del=menu.addAction("🗑 Удалить  [Del]")
            a_del.triggered.connect(lambda:self._local_delete())
            menu.addSeparator()
        a_new=menu.addAction("📁 Новая папка")
        a_new.triggered.connect(self._local_mkdir)
        a_ref=menu.addAction("🔄 Обновить")
        a_ref.triggered.connect(self._refresh_local)
        menu.exec_(QCursor.pos())

    def keyPressEvent(self,e):
        if self._is_restore_tab_active():
            if e.key()==Qt.Key_F2: self._local_rename(); return
            if e.key()==Qt.Key_Delete: self._local_delete(); return
        super().keyPressEvent(e)

    def _local_mkdir(self):
        name,ok=QInputDialog.getText(self,"Новая папка","Имя папки:")
        if not ok or not name: return
        new=self._local_path/name
        try: new.mkdir(); self._refresh_local()
        except Exception as e: QMessageBox.warning(self,"Ошибка",str(e))

    def _local_rename(self):
        it=self.local_tree.currentItem()
        if not it: return
        p=Path(it.data(0,Qt.UserRole) or "")
        new_name,ok=QInputDialog.getText(self,"Переименовать","Новое имя:",text=p.name)
        if not ok or not new_name or new_name==p.name: return
        try: p.rename(p.parent/new_name); self._refresh_local()
        except Exception as e: QMessageBox.warning(self,"Ошибка",str(e))

    def _local_delete(self):
        items=self.local_tree.selectedItems()
        if not items: return
        names=", ".join(Path(it.data(0,Qt.UserRole)).name for it in items)
        if QMessageBox.question(self,"Удалить?",
            f"Удалить {len(items)} объект(ов)?\n{names}") != QMessageBox.Yes: return
        for it in items:
            p=Path(it.data(0,Qt.UserRole) or "")
            try:
                if p.is_dir(): shutil.rmtree(str(p))
                else: p.unlink()
            except Exception as e: QMessageBox.warning(self,"Ошибка",str(e))
        self._refresh_local()

    # ─── SOURCES ─────────────────────────────────────────────────────────────

    def _add_src(self):
        p=QFileDialog.getExistingDirectory(self,"Папка для бэкапа")
        if p: self.src_list.addItem(p.replace("/","\\")); self._autosave(); self._saved()

    def _add_src_files(self):
        # Shift/Ctrl — мультивыбор в стандартном диалоге
        paths, _ = QFileDialog.getOpenFileNames(self, "Файлы для бэкапа")
        existing = {self.src_list.item(i).text() for i in range(self.src_list.count())}
        added = 0
        for p in paths:
            p = p.replace("/", "\\")
            if p not in existing:
                self.src_list.addItem(p); existing.add(p); added += 1
        if added: self._autosave(); self._saved()

    def _edit_src(self):
        it=self.src_list.currentItem()
        if not it: return
        new,ok=QInputDialog.getText(self,"Редактировать","Путь:",text=it.text())
        if ok and new: it.setText(new); self._autosave(); self._saved()

    def _del_src(self):
        r=self.src_list.currentRow()
        if r>=0: self.src_list.takeItem(r); self._autosave(); self._saved()

    def _import_sources_from_file(self):
        path, _ = QFileDialog.getOpenFileName(self, "Выбрать файл со списком путей", "",
                                              "Текстовые файлы (*.txt);;Все файлы (*)")
        if not path:
            return
        try:
            text = Path(path).read_text(encoding="utf-8-sig", errors="replace")
        except Exception as e:
            QMessageBox.warning(self, "Импорт", f"Не удалось прочитать файл:\n{e}"); return
        existing = {self.src_list.item(i).text() for i in range(self.src_list.count())}
        added = 0
        for line in text.splitlines():
            p = line.lstrip("﻿").strip().strip('"\'')
            if p and p not in existing:
                self.src_list.addItem(p); existing.add(p); added += 1
        if added:
            self._autosave(); self.statusBar().showMessage(f"✅ Добавлено {added} путей из файла", 4000)
        else:
            self.statusBar().showMessage("ℹ️ Новых путей не найдено", 3000)

    # ─── BACKUP ──────────────────────────────────────────────────────────────

    def _set_bk_status_line(self, text: str = "", ok: Optional[bool] = None):
        text = str(text or "").strip()
        self.lbl_bk_status.setVisible(bool(text))
        if not text:
            self.lbl_bk_status.clear()
            return
        self.lbl_bk_status.setText(text)
        if ok is True:
            self.lbl_bk_status.setStyleSheet("font-weight:bold;font-size:12px;color:#006600;")
        elif ok is False:
            self.lbl_bk_status.setStyleSheet("font-weight:bold;font-size:12px;color:#cc0000;")
        else:
            self.lbl_bk_status.setStyleSheet("font-weight:bold;font-size:12px;color:#333;")

    def _set_bk_vss_status(self, ok: Optional[bool]):
        self._bk_vss_ok = ok
        if ok is True:
            self.lbl_bk_vss.setText("Shadow Copy = On")
            self.lbl_bk_vss.setStyleSheet("color:#006600;font-size:10.5pt;font-weight:bold;")
        elif ok is False:
            self.lbl_bk_vss.setText("Shadow Copy = Off")
            self.lbl_bk_vss.setStyleSheet("color:#888;font-size:10.5pt;")
        else:
            self.lbl_bk_vss.setText("Shadow Copy = —")
            self.lbl_bk_vss.setStyleSheet("color:#aaa;font-size:10.5pt;")

    def _load_last_run_stats(self):
        pid = getattr(self, "pid", None)
        if not pid or self._backup_running:
            return
        import threading
        def _fetch():
            try:
                data = agent_api_request(f"/last-run/{pid}", timeout=3)
                if not isinstance(data, dict) or not data.get("status"):
                    return
                status = data.get("status", "")
                ts = data.get("timestamp", "")
                try:
                    from datetime import datetime, timezone
                    dt = datetime.fromisoformat(ts)
                    time_s = dt.strftime("%H:%M")
                except Exception:
                    time_s = ""
                dur = int(data.get("duration_s", 0))
                fn = int(data.get("files_new", 0))
                ba = int(data.get("bytes_added", 0))
                snap = str(data.get("snapshot_id") or "")[:8]
                ec = int(data.get("error_count", 0))
                if status == "success":
                    parts = []
                    if time_s: parts.append(time_s)
                    if fn: parts.append(f"+{fn} файл{'ов' if fn%10!=1 or fn//10%10==1 else ''}")
                    if ba: parts.append(_fmt_bytes_ui(ba))
                    if dur: parts.append(f"{dur}с")
                    if snap: parts.append(f"snap {snap}")
                    line = "✓ " + " · ".join(parts) if parts else "✓ Бэкап выполнен"
                    style = "color:#006600;font-size:9.5pt;"
                else:
                    parts = []
                    if time_s: parts.append(time_s)
                    if ec: parts.append(f"{ec} ошибок")
                    line = "✗ " + " · ".join(parts) if parts else "✗ Ошибка бэкапа"
                    style = "color:#cc0000;font-size:9.5pt;"
                def _apply():
                    if not self._backup_running and getattr(self, "pid", None) == pid:
                        self.lbl_bk_stats.setText(line)
                        self.lbl_bk_stats.setStyleSheet(style)
                self._ui_call_signal.emit(_apply)
            except Exception:
                pass
        threading.Thread(target=_fetch, daemon=True).start()

    def _set_bk_result_status(self, state: str = "idle"):
        state = str(state or "idle").strip().lower()
        if state == "running":
            self.lbl_bk_result.setText("Статус: Выполняется…")
            self.lbl_bk_result.setStyleSheet("color:#333;font-size:10.5pt;")
            return
        if state in ("ok", "error"):
            ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        if state == "ok":
            self.lbl_bk_result.setText(f"Статус: Завершён ({ts})")
            self.lbl_bk_result.setStyleSheet("color:#006600;font-size:10.5pt;font-weight:bold;")
            return
        if state == "error":
            self.lbl_bk_result.setText(
                f"Статус: <a href='error'><span style='color:#cc0000; text-decoration: underline; font-weight: bold;'>Завершён с ошибками</span></a> ({ts})"
            )
            self.lbl_bk_result.setStyleSheet("color:#cc0000;font-size:10.5pt;font-weight:bold;")
            return
        self.lbl_bk_result.setText("Статус: —")
        self.lbl_bk_result.setStyleSheet("color:#333;font-size:10.5pt;")

    def _show_backup_error_logs_popup(self, _link: str = "error"):
        if not self.pid:
            return
        text = "".join(self.cfg.get_logs(self.pid, mode="error")).strip() or "Ошибок бэкапа нет."
        dlg = QDialog(self)
        dlg.setWindowTitle("Логи ошибок бэкапа")
        dlg.resize(920, 560)
        ly = QVBoxLayout(dlg)
        view = QTextEdit()
        view.setReadOnly(True)
        view.setFont(QFont("Consolas", 9))
        view.setPlainText(text)
        close_btn = QPushButton("Закрыть")
        close_btn.clicked.connect(dlg.accept)
        ly.addWidget(view, 1)
        ly.addWidget(close_btn, 0, Qt.AlignRight)
        dlg.exec_()

    def _on_backup_output(self, line: str):
        if self.sender() is not self.worker:
            # Бэкап невыбранного профиля: UI не трогаем, но строки для
            # web-отчёта копим — иначе отчёт уйдёт пустым.
            if self._backup_pid and self.sender() is self._workers.get(self._backup_pid):
                if not line.strip().startswith(('{"message_type":"status"',
                                                '{"message_type": "status"')):
                    self._remember_backup_report_line(line)
            return
        # status JSON → только прогресс, никогда в лог
        if line.strip().startswith('{"message_type":"status"') or \
           line.strip().startswith('{"message_type": "status"'):
            self._parse_bk_line(line)
            return
        self._remember_backup_report_line(line)
        self._log(line, channel="info")
        self._parse_bk_line(line)

    def _on_backup_error(self, line: str):
        if self.sender() is not self.worker:
            if self._backup_pid and self.sender() is self._workers.get(self._backup_pid):
                self._remember_backup_report_line("❌ " + line)
            return
        low = line.lower()
        is_vss = "vss" in low or "shadow copy" in low or "shadow snapshot" in low
        if is_vss:
            vss_fail = any(x in low for x in ("error", "fail", "could not", "unable", "exception"))
            if "snapshot_set_in_progress" in low or "snapshot set in progress" in low:
                # Фиксим только если мы — единственный restic.exe.
                # Если их несколько — снепшот занят нашим же другим бэкапом, не трогаем.
                if self._count_restic_processes() <= 1:
                    self._bk_vss_stuck = True
                else:
                    self._log("⚠ VSS_E_SNAPSHOT_SET_IN_PROGRESS — другой бэкап уже держит снепшот, сброс VSS пропущен", "INFO", channel="info")
            self._set_bk_vss_status(False if vss_fail else True)
            msg = ("❌ VSS: " if vss_fail else "VSS: ") + line
            self._remember_backup_report_line(msg)
            self._log(msg, "ERROR" if vss_fail else "INFO", channel="error")
        else:
            self._parse_bk_line(line)
            msg = "❌ " + line
            self._remember_backup_report_line(msg)
            self._log(msg, "ERROR", channel="error")

    def _remember_backup_report_line(self, line: str):
        if not line:
            return
        self._bk_report_lines.append(line)
        overflow = len(self._bk_report_lines) - MAX_BACKUP_REPORT_LINES
        if overflow > 0:
            del self._bk_report_lines[:overflow]

    def _send_backup_web_report(self, code: int, pid: str = ""):
        pid = pid or self.pid
        if not pid:
            return
        url = (self.cfg.report_url or "").strip()
        if not url:
            return
        prof = self.cfg.profile(pid) or {}
        profile_name = prof.get("name") or pid
        sources = prof.get("sources", [])
        dest = prof.get("repo_path", "")
        report = build_backup_report(
            profile_name,
            pid,
            code,
            self._bk_started_at,
            self._bk_report_lines,
            _agent_ver_str(),
            source_paths=sources,
            dest_path=dest,
        )
        if not web_report_send(url, report, self.cfg.report_api_key):
            self._log("⚠ Не удалось отправить web-отчёт о backup", "ERROR")

    def _bk_ui(self,msg,running=False,ok=None):
        """Update backup status UI."""
        # Don't leak another profile's backup state into the current profile view
        owns_backup = self._backup_running and self._backup_pid == self.pid
        if running and not owns_backup:
            return
        if running:
            self._set_bk_status_line("")
            self.lbl_bk_vss.setVisible(True)
        else:
            self.lbl_bk_vss.setVisible(False)
            if msg.strip() in ("", "Готов", "Готов к запуску"):
                self._set_bk_status_line("Готов к запуску" if msg.strip() == "Готов к запуску" else "")
            else:
                self._set_bk_status_line(msg, ok=ok)
        self.prog_bk.setVisible(running)
        self.btn_bk.setVisible(not running)
        self.bk_ctrl_row.setVisible(running)
        if not running:
            self.lbl_bk_file.setText(""); self.lbl_bk_stats.setText("")
            self.btn_bk_pause.setText("⏸ Пауза")
            self.btn_bk_pause.setStyleSheet("font-weight:bold;color:#444;background:#fff8dd;border:1px solid #ddcc88;")

    def _parse_bk_line(self,line:str):
        """Parse restic backup output lines for progress info."""
        low = line.lower()
        if "vss" in low or "shadow copy" in low or "shadow snapshot" in low:
            vss_fail = any(x in low for x in ("error", "fail", "could not", "unable", "exception"))
            self._set_bk_vss_status(False if vss_fail else True)
            return
        # Try JSON (--json mode)
        stripped = line.strip()
        if stripped.startswith("{"):
            try:
                d = json.loads(stripped)
                mt = d.get("message_type", "")
                if mt == "status":
                    pct = d.get("percent_done", 0) * 100
                    fd = d.get("files_done", 0)
                    tf = d.get("total_files", 0)
                    bd = d.get("bytes_done", 0)
                    tb = d.get("total_bytes", 0) or 0
                    cur = (d.get("current_files") or [""])[0]
                    # При --no-scan restic не делает предсканирование, total_files/total_bytes=0.
                    # Показываем только текущие значения, чтобы не было "29639/0 файлов".
                    if tf > 0:
                        files_part = f"{fd}/{tf} файлов"
                    else:
                        files_part = f"{fd} файлов"
                    if tb > 0:
                        size_part = f"  {_fmt_bytes_ui(bd)}/{_fmt_bytes_ui(tb)}"
                        head = f"{pct:.0f}% — "
                    else:
                        size_part = f"  {_fmt_bytes_ui(bd)}"
                        head = ""  # без процента — он 0 пока нет total
                    self.lbl_bk_stats.setText(head + files_part + size_part)
                    if cur:
                        self.lbl_bk_file.setText(cur[:120])
                    if pct > 0:
                        if self.prog_bk.maximum() != 100:
                            self.prog_bk.setRange(0, 100)
                        self.prog_bk.setValue(int(pct))
                    else:
                        if self.prog_bk.minimum() != 0 or self.prog_bk.maximum() != 0:
                            self.prog_bk.setRange(0, 0)
                    import time as _time
                    _now = _time.monotonic()
                    if _now - getattr(self, '_bk_last_stats_log', 0.0) >= 60:
                        self._bk_last_stats_log = _now
                        self._log(f"Backup: {head}{files_part}{size_part}", channel="info")
                elif mt == "summary":
                    fn = d.get("files_new", 0)
                    fc = d.get("files_changed", 0)
                    da = d.get("data_added", 0)
                    self.lbl_bk_stats.setText(f"Новых: {fn}  Изменено: {fc}  Добавлено: {_fmt_bytes_ui(da)}")
                    self.lbl_bk_file.setText("")
                return
            except Exception:
                pass
        # Fallback: human-readable output
        if line.startswith("Files:") or "new," in line:
            self.lbl_bk_stats.setText(line.strip())
        elif line.startswith("Added") or line.startswith("processed") or "MiB" in line or "KiB" in line:
            self.lbl_bk_stats.setText(line.strip())
        elif "backed up" in line.lower() or "snapshot" in line.lower():
            self.lbl_bk_stats.setText(line.strip())
        else:
            if len(line) > 5 and not line.startswith("[") and not line.startswith("scan"):
                self.lbl_bk_file.setText(line[:120])

    def _reset_runtime_log_views(self):
        for name in ("log_info_view", "log_debug_view", "log_error_view"):
            view = getattr(self, name, None)
            if view is not None:
                view.clear()
        self._logs_dirty = False

    def _start_backup_preflight(self):
        prof=self.cfg.profile(self.pid) or {}
        self._bk_started_at=None
        self._bk_report_lines=[]
        self._post_backup_code=None
        self._bk_last_stats_log = 0.0
        # Сбрасываем глобальный флаг только если он относится к текущему профилю,
        # иначе при старте второго бэкапа мы затрём состояние первого.
        if not self._backup_running or self._backup_pid == self.pid:
            self._backup_running = False
        self._bk_vss_stuck = False
        self._bk_vss_retry_done = False
        # Layer 0: захват глобального mutex по репо до любых действий
        repo = prof.get("repo_path", "") or ""
        if repo and not self._repo_mutex_held:
            self._acquire_repo_mutex(repo, self._continue_backup_preflight)
            return
        self._continue_backup_preflight()

    def _acquire_repo_mutex(self, repo: str, on_acquired):
        try:
            if self._repo_mutex is not None:
                self._repo_mutex.release()
        except Exception:
            pass
        m = RepoMutex(repo, self)
        self._repo_mutex = m
        self._repo_mutex_held = False
        def _current():
            return self._repo_mutex is m and not m._release_event.is_set()
        def _continue_if_current():
            if _current():
                on_acquired()
        def _on_wait():
            if not _current():
                return
            self._bk_ui("⏳ Репозиторий занят — жду освобождения…", running=True)
            self._log("RepoMutex: репо занят другим бэкапом, жду", channel="info")
        def _on_acq():
            if not _current():
                return
            self._repo_mutex_held = True
            self._log("RepoMutex: захвачен", channel="info")
            QTimer.singleShot(0, _continue_if_current)
        def _on_fail(err):
            if not _current():
                return
            self._bk_ui(f"❌ Не удалось захватить mutex репозитория: {err}", ok=False)
            self._log(f"RepoMutex недоступен ({err}) — бэкап остановлен", "ERROR", channel="error")
            self.btn_bk.setEnabled(True)
            self._release_repo_mutex()
        m.waiting.connect(_on_wait)
        m.acquired.connect(_on_acq)
        m.failed.connect(_on_fail)
        m.acquire_async()

    def _release_repo_mutex(self):
        try:
            if self._repo_mutex is not None:
                self._repo_mutex.release()
                self._log("RepoMutex: освобождён", channel="info")
        except Exception:
            pass
        self._repo_mutex = None
        self._repo_mutex_held = False
        try:
            if self._heartbeat is not None:
                self._heartbeat.stop()
                self._heartbeat = None
        except Exception:
            pass

    def _continue_backup_preflight(self):
        prof = self.cfg.profile(self.pid) or {}
        if self.pid:
            self.cfg.clr_logs(self.pid)
        self._reset_runtime_log_views()
        self._set_bk_vss_status(True if prof.get("use_vss", True) else False)
        self._set_bk_result_status("running")
        # Wait if backup already running for this profile
        existing = self._workers.get(self.pid)
        if existing and existing.proc.state() != QProcess.NotRunning:
            self._bk_ui("⏳ Ожидание завершения предыдущего бэкапа…",running=True)
            self.btn_bk.setEnabled(False)
            QTimer.singleShot(2000, self._continue_backup_preflight)
            return
        self.btn_bk.setEnabled(False)
        self._start_res_monitor()
        if self._backup_timeout_timer:
            self._backup_timeout_timer.stop()
        global_timeout = self.cfg.global_backup_timeout_hours
        if global_timeout > 0:
            self._backup_timeout_timer = QTimer(self)
            self._backup_timeout_timer.setInterval(60000)  # check every minute
            self._backup_timeout_timer.timeout.connect(self._check_backup_timeout)
            self._backup_timeout_timer.start()
        prof=self.cfg.profile(self.pid) or {}
        if not prof.get("pre_unlock", True) and not prof.get("force_unlock_orphan", True):
            self._run_pre_backup_steps()
            return
        self._bk_ui("⏳ Smart preflight: анализирую локи…",running=True)
        self._log("Smart preflight: list locks + cat lock", channel="info")
        repo = prof.get("repo_path", "") or ""
        pw   = prof.get("repo_password", "") or ""
        if not repo or not pw:
            self._on_pre_backup_unlock_done(0, "")
            return
        grace = int(prof.get("stale_grace_minutes", self.cfg.global_stale_grace_minutes if hasattr(self.cfg, "global_stale_grace_minutes") else 35) or 35)
        force = bool(prof.get("force_unlock_orphan", prof.get("pre_unlock", True)))
        lp = LockPreflight(self.cfg.restic, repo, pw, grace_minutes=grace, force_orphan=force, parent=self)
        self._lock_preflight = lp
        lp.done.connect(self._on_smart_preflight_done)
        lp.run()

    # ─── WOL / выключение: автосохранение и кнопки «Проверить» ───────────────

    _POWER_KEYS = ("wol_enabled", "wol_mac", "wol_host", "wol_ping_interval", "wol_wait",
                   "wol_wait_timeout", "shutdown_after_backup", "shutdown_delay",
                   "shutdown_remote", "shutdown_remote_mac")

    def _power_status(self, lbl, text: str, ok: bool = True):
        lbl.setText(text); lbl.setVisible(True); lbl.setStyleSheet("color:#006600;" if ok else "color:#cc0000;")

    def _save_power(self, *_):
        """Поля WOL/выключения пишутся в профиль сразу, отдельно от «Сохранить» репозитория."""
        if getattr(self, "_power_loading", False) or not self.pid:
            return
        for ed, lbl in ((self.edit_wol_mac, self.lbl_wol_status), (self.edit_shutdown_mac, self.lbl_sd_status)):
            raw = ed.text().strip()
            mac = normalize_mac(raw)
            if mac and mac != raw:
                ed.setText(mac)
            if raw and not mac:
                self._power_status(lbl, f"❌ «{raw}» — не MAC-адрес (нужно 12 hex-цифр)", False)
            elif mac and self.sender() is ed:
                self._power_status(lbl, f"✅ MAC {mac} сохранён")
        prof = dict(self.cfg.profile(self.pid) or {})
        ui = self._repo_data_from_ui()
        prof.update({k: ui[k] for k in self._POWER_KEYS})
        self.cfg.save_profile(self.pid, prof)
        self.statusBar().showMessage("✅ Настройки WOL/выключения сохранены", 3000)

    def _wol_mac_by_ip(self):
        host = self.edit_wol_host.text().strip()
        if not host:
            self._power_status(self.lbl_wol_status, "❌ Впишите IP или имя ПК", False); return
        self._power_status(self.lbl_wol_status, f"⏳ Ищу MAC для {host}…")
        def _run():
            mac = ""
            try:
                ip = socket.gethostbyname(host)
                self._host_reachable(ip, 2)  # пинг кладёт хост в ARP-таблицу
                out = subprocess.run(["arp", "-a", ip], capture_output=True, text=True, errors="replace",
                                     timeout=10, **hidden_console_subprocess_kwargs()).stdout
                for line in out.splitlines():
                    parts = line.split()
                    if len(parts) >= 2 and parts[0] == ip:
                        mac = normalize_mac(parts[1])
            except Exception:
                pass
            def _done():
                if mac:
                    self.edit_wol_mac.setText(mac)
                    self._save_power()
                    self._power_status(self.lbl_wol_status, f"✅ {host}: MAC {mac} подставлен и сохранён")
                else:
                    self._power_status(self.lbl_wol_status, f"❌ MAC для {host} не найден: ПК должен быть "
                                       "включён и в той же сети (через роутер MAC не виден)", False)
            self._ui_call_signal.emit(_done)
        threading.Thread(target=_run, daemon=True).start()

    def _wol_test(self):
        self._save_power()
        mac = normalize_mac(self.edit_wol_mac.text())
        if not mac:
            self._power_status(self.lbl_wol_status, "❌ Введите MAC-адрес", False); return
        host = self.edit_wol_host.text().strip()
        if not send_wol(mac, host):
            self._power_status(self.lbl_wol_status, "❌ Не удалось отправить magic packet", False); return
        target = host or mac_to_ip(mac)
        if target:
            open_ping_window(target)
            self._power_status(self.lbl_wol_status, f"✅ Magic packet → {mac}, пингую {target}")
        else:
            self._power_status(self.lbl_wol_status, f"✅ Magic packet → {mac}. IP неизвестен — "
                               "заполните «IP / Имя хоста», чтобы открыть пинг", False)
        self._log(f"WOL (проверка): magic packet → {mac}", channel="info")

    def _shutdown_abort(self):
        self._power_cmd(["shutdown", "/a"], "✅ Выключение отменено")

    def _power_cmd(self, args, ok_text):
        def _run():
            try:
                r = subprocess.run(args, capture_output=True, text=True, encoding="oem", errors="replace", timeout=60,
                                   **hidden_console_subprocess_kwargs())
                rc, detail = r.returncode, ((r.stderr or "") + (r.stdout or "")).strip()
            except Exception as e:
                rc, detail = -1, str(e)
            self._ui_call_signal.emit(lambda: self._power_status(
                self.lbl_sd_status, ok_text if rc == 0 else f"❌ Код {rc}: {detail}", rc == 0))
        threading.Thread(target=_run, daemon=True).start()

    def _shutdown_remote_test(self):
        self._save_power()
        mac = normalize_mac(self.edit_shutdown_mac.text())
        if not mac:
            self._power_status(self.lbl_sd_status, "❌ Введите MAC-адрес другого ПК", False); return
        target = resolve_power_target(mac, self.cfg.profile(self.pid) or {})
        if QMessageBox.question(self, "Выключение", f"Отправить пакет выключения на {mac}"
                                f"{' (' + target + ')' if target else ''}?") != QMessageBox.Yes:
            return
        if not send_sleep(mac, target):
            self._power_status(self.lbl_sd_status, "❌ Не удалось отправить пакет", False); return
        self._log(f"Sleep-on-LAN (проверка): пакет выключения → {mac}", channel="info")
        if target:
            open_ping_window(target)
        self._power_status(self.lbl_sd_status, f"✅ Пакет выключения → {mac}. ПК выключится через "
                           f"{SLEEP_ON_LAN_DELAY} сек, если на нём RestLex-агент с галкой «Выключение по сети» "
                           f"в Настройках" + ("" if target else " (IP не найден — пинг не открыт)"))

    def _host_reachable(self, host: str, timeout: int = 5) -> bool:
        try:
            kw = {"capture_output": True, "text": True, "timeout": timeout}
            kw.update(hidden_console_subprocess_kwargs())
            if sys.platform == "win32":
                r = subprocess.run(["ping", "-n", "1", "-w", str(timeout * 1000), host], **kw)
            else:
                r = subprocess.run(["ping", "-c", "1", "-W", str(timeout), host], **kw)
            return r.returncode == 0
        except Exception:
            return False

    def _wol_wait_async(self, host: str, on_success, on_fail,
                        interval: int = WOL_PING_INTERVAL_DEF,
                        total_timeout: int = WOL_WAIT_TIMEOUT_DEF):
        ev = threading.Event()
        result = [False, 0]
        def _thread():
            deadline = time.monotonic() + total_timeout
            attempt = 0
            while True:
                attempt += 1
                started = time.monotonic()
                # таймаут одного ping не должен вылезать за общий дедлайн
                ping_to = max(1, min(interval, int(deadline - started)))
                if self._host_reachable(host, timeout=ping_to):
                    result[0] = True
                    result[1] = attempt
                    break
                if time.monotonic() >= deadline:
                    break
                # ping мог вернуться мгновенно (unreachable) — добираем интервал
                time.sleep(max(0.0, interval - (time.monotonic() - started)))
            ev.set()
        threading.Thread(target=_thread, daemon=True).start()
        t = QTimer(self)
        t.setInterval(500)
        def _check():
            if ev.is_set():
                t.stop()
                if result[0]:
                    self._log(f"Хост {host} доступен (попытка {result[1]})", channel="info")
                    on_success()
                else:
                    on_fail()
        t.timeout.connect(_check)
        t.start()

    def _run_pre_backup_steps(self):
        prof = self.cfg.profile(self.pid) or {}
        if prof.get("wol_enabled", False):
            mac = str(prof.get("wol_mac", "") or "").strip()
            host = str(prof.get("wol_host", "") or "").strip()
            if mac:
                self._bk_ui("⏳ Wake-on-LAN…", running=True)
                self._log(f"WOL: будим {host or '?'} ({mac})", channel="info")
                if send_wol(mac, host):
                    self._log("Magic packet отправлен", channel="info")
                else:
                    self._log(f"❌ Не удалось отправить WOL (MAC «{mac}» некорректен?)", "ERROR", channel="error")
                if not host or not prof.get("wol_wait", True):
                    self._log("WOL: ожидание выключено или IP не задан — продолжаю бэкап", channel="info")
                    self._run_pre_backup_cmds_and_beyond()
                    return
                interval = int(prof.get("wol_ping_interval", WOL_PING_INTERVAL_DEF) or WOL_PING_INTERVAL_DEF)
                timeout_s = int(prof.get("wol_wait_timeout", WOL_WAIT_TIMEOUT_DEF) or WOL_WAIT_TIMEOUT_DEF)
                self._bk_ui(f"⏳ Ожидание {host}…", running=True)
                def _on_wol_ok():
                    self._run_pre_backup_cmds_and_beyond()
                def _on_wol_fail():
                    self._log(f"❌ Хост {host} не отвечает после WOL", "ERROR", channel="error")
                    self._bk_ui(f"❌ Хост {host} не отвечает после WOL", ok=False)
                    self._set_bk_result_status("error")
                    self.btn_bk.setEnabled(True)
                    self._ref_logs()
                    tg_fatal = prof.get("tg_fatal", self.cfg.tg_fatal)
                    if tg_fatal:
                        threading.Thread(target=tg_broadcast, args=(
                            self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group,
                            f"RestLex - Профиль: {self._pname()} | Ошибка: WOL — {host} не отвечает"),
                            daemon=True).start()
                self._wol_wait_async(host, _on_wol_ok, _on_wol_fail,
                                     interval=interval, total_timeout=timeout_s)
                return
        self._run_pre_backup_cmds_and_beyond()

    def _run_pre_backup_cmds_and_beyond(self):
        prof = self.cfg.profile(self.pid) or {}
        raw_cmd = str(self.cfg.global_pre_backup_cmd or "").strip()
        if raw_cmd:
            cmds = [line.strip() for line in raw_cmd.splitlines() if line.strip()]
            self._run_pre_cmds(cmds, 0, prof)
            return
        if prof.get("pre_repair_index", False):
            self._bk_ui("⏳ Repair index…", running=True)
            self._log("=== REPAIR INDEX ===", channel="info")
            w = self._mk_worker()
            if not w:
                self.btn_bk.setEnabled(True); return
            self._workers[self.pid] = w
            w.out.connect(lambda m: self._log(m, channel="info"))
            w.err.connect(lambda m: self._log("❌ "+m, "ERROR", channel="error"))
            w.done.connect(lambda c, _: self._on_pre_maintenance_done(c, "repair index"))
            w.repair_index(); return
        self._run_pre_rebuild_or_backup()

    def _run_pre_cmds(self, cmds: list, idx: int, prof: dict):
        if idx >= len(cmds):
            # All pre-backup CMDs passed — continue normal chain
            if prof.get("pre_repair_index", False):
                self._bk_ui("⏳ Repair index…", running=True)
                self._log("=== REPAIR INDEX ===", channel="info")
                w = self._mk_worker()
                if not w:
                    self.btn_bk.setEnabled(True); return
                self._workers[self.pid] = w
                w.out.connect(lambda m: self._log(m, channel="info"))
                w.err.connect(lambda m: self._log("❌ "+m, "ERROR", channel="error"))
                w.done.connect(lambda c, _: self._on_pre_maintenance_done(c, "repair index"))
                w.repair_index()
            else:
                self._run_pre_rebuild_or_backup()
            return
        cmd = cmds[idx]
        total = len(cmds)
        self._bk_ui(f"⏳ Pre-backup CMD {idx+1}/{total}…", running=True)
        self._log(f"Pre-backup CMD: {cmd}", channel="info")
        result_holder: list = [None]
        done_event = threading.Event()

        def _thread():
            try:
                r = subprocess.run(
                    cmd, shell=True, capture_output=True, text=True, timeout=600,
                    encoding="utf-8", errors="replace",
                    **hidden_console_subprocess_kwargs()
                )
                result_holder[0] = r
            except Exception as exc:
                result_holder[0] = exc
            finally:
                done_event.set()

        threading.Thread(target=_thread, daemon=True).start()

        _poll_timer = QTimer(self)

        def _check():
            if not done_event.is_set():
                return
            _poll_timer.stop()
            res = result_holder[0]
            if isinstance(res, Exception):
                self._log(f"❌ Pre-backup CMD error: {res}", "ERROR", channel="error")
                self._remember_backup_report_line(f"❌ Pre-backup CMD error: {res}")
                threading.Thread(target=self._send_backup_web_report, args=(-1,), daemon=True).start()
                self._bk_ui(f"❌ Ошибка команды перед бэкапом: {res}", ok=False)
                self._set_bk_result_status("error")
                self.btn_bk.setEnabled(True)
                self._ref_logs()
                return
            if res.stdout:
                for ln in res.stdout.splitlines():
                    self._log(ln, channel="info")
            if res.stderr:
                for ln in res.stderr.splitlines():
                    self._log("❌ "+ln, "ERROR", channel="error")
                    self._remember_backup_report_line("❌ " + ln)
            if res.returncode != 0:
                fail_msg = f"Pre-backup CMD завершилась с кодом {res.returncode}"
                self._log(fail_msg, "ERROR", channel="error")
                self._remember_backup_report_line(f"❌ {fail_msg}")
                threading.Thread(target=self._send_backup_web_report, args=(-1,), daemon=True).start()
                self._bk_ui(f"❌ CMD завершилась с ошибкой (код {res.returncode})", ok=False)
                self._set_bk_result_status("error")
                self.btn_bk.setEnabled(True)
                self._ref_logs()
                tg_fatal = prof.get("tg_fatal", self.cfg.tg_fatal)
                if tg_fatal:
                    threading.Thread(target=tg_broadcast, args=(
                        self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group,
                        f"RestLex - Профиль: {self._pname()} | Ошибка: Pre-backup CMD (код {res.returncode})"),
                        daemon=True).start()
                return
            self._run_pre_cmds(cmds, idx + 1, prof)

        _poll_timer.timeout.connect(_check)
        _poll_timer.start(200)

    def _run_pre_rebuild_or_backup(self):
        prof = self.cfg.profile(self.pid) or {}
        if prof.get("use_vss", True) and sys.platform == "win32":
            self._vss_preflight()
        else:
            self._do_bk()

    def _vss_preflight(self):
        """Проверяем VSS writer'ов перед бэкапом. Если есть Неисправен → чиним до старта."""
        self._bk_ui("⏳ Проверка VSS...", running=True)
        self._log("Проверка состояния VSS writer'ов...", channel="info")
        result: list = [None]
        done = threading.Event()

        def _check():
            try:
                r = subprocess.run(
                    ["vssadmin", "list", "writers"],
                    capture_output=True, timeout=8,
                    **hidden_console_subprocess_kwargs()
                )
                # OEM/CP866 на русских Windows — декодируем явно
                enc = "oem" if sys.platform == "win32" else "utf-8"
                out = (r.stdout or b"").decode(enc, errors="replace")
                # Считаем только строки с состоянием [8] (ASCII, без проблем с кодировкой)
                count = sum(1 for ln in out.splitlines() if "[8]" in ln)
                result[0] = count
            except subprocess.TimeoutExpired:
                result[0] = -1  # -1 = VSS завис
            except Exception:
                result[0] = 0
            done.set()

        threading.Thread(target=_check, daemon=True).start()
        timer = QTimer(self)
        timer.setSingleShot(False)

        def _poll():
            if not done.is_set():
                return
            timer.stop()
            count = result[0] or 0
            if count != 0:
                label = "VSS не отвечает" if count == -1 else f"{count} writer(ов) неисправно"
                self._log(f"⚠ VSS: {label} — запускаю авто-сброс", channel="info")
                threading.Thread(target=self._vss_fix_and_continue, daemon=True).start()
            else:
                self._log("VSS writer'ы в норме", channel="info")
                self._do_bk()

        timer.timeout.connect(_poll)
        timer.start(200)

    def _vss_fix_and_continue(self):
        """Перезапускаем VSS, затем удаляем несогласованные тени, запускаем бэкап."""
        try:
            kw = hidden_console_subprocess_kwargs()
            self._log("🔧 Останавливаю VSS...", channel="info")
            subprocess.run(["net", "stop", "vss", "/y"], capture_output=True, timeout=15, **kw)
            time.sleep(2)
            self._log("🔧 Запускаю VSS...", channel="info")
            subprocess.run(["net", "start", "vss"], capture_output=True, timeout=15, **kw)
            time.sleep(3)
            self._log("🔧 Удаляю несогласованные теневые копии...", channel="info")
            subprocess.run(["vssadmin", "delete", "shadows", "/all", "/quiet"],
                           capture_output=True, timeout=15, **kw)
            self._log("✅ VSS восстановлен, запускаю бэкап", channel="info")
        except Exception as e:
            self._log(f"⚠ VSS fix ошибка: {e}", "ERROR", channel="info")
        # Мы В ФОНОВОМ ПОТОКЕ: QTimer.singleShot(0, …) здесь молча теряется
        # (нет Qt event loop) — бэкап никогда не стартовал, UI навсегда «Проверка VSS».
        self._ui_call_signal.emit(self._do_bk)

    def _on_pre_maintenance_done(self, code, name):
        if self.sender() is not self.worker:
            return
        self._log(f"{name} завершён с кодом {code}", channel="info")
        if code != 0:
            self._bk_ui(f"⚠️ {name} завершился с ошибкой (код {code})",ok=False)
            self.btn_bk.setEnabled(True)
            self._ref_logs()
            prof=self.cfg.profile(self.pid) or {}
            tg_prune=prof.get("tg_prune", self.cfg.tg_prune)
            if tg_prune:
                threading.Thread(target=tg_broadcast,args=(self.cfg.tg_token,self.cfg.tg_chat,self.cfg.tg_group,
                    f"RestLex - Профиль: {self._pname()} | Ошибка: {name}"),daemon=True).start()
            return
        if name == "repair index":
            self._run_pre_rebuild_or_backup()
        else:
            self._do_bk()

    def _on_smart_preflight_done(self, code: int, message: str):
        # Поздний сигнал от отменённой/предыдущей проверки не должен запускать бэкап.
        lp = getattr(self, "_lock_preflight", None)
        if lp is None or self.sender() is not lp or lp._cancelled.is_set():
            return
        self._lock_preflight = None
        # 0=OK, 1=ABORT (активный бэкап), 2=WAIT (чужой/непроверенный lock), 99=error
        if code == 0:
            if message and message != "PROCEED":
                self._log(f"Smart preflight: {message}", channel="info")
                prof = self.cfg.profile(self.pid) or {}
                if message.startswith("сняты локи") and prof.get("tg_prune", self.cfg.tg_prune):
                    threading.Thread(target=tg_broadcast, args=(
                        self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group,
                        f"RestLex - Профиль: {self._pname()} | LockReleased_OrphanRemoved: {message}"),
                        daemon=True).start()
            self._run_pre_backup_steps()
            return
        prof = self.cfg.profile(self.pid) or {}
        if code == 1:
            self._set_bk_result_status("error")
            self._bk_ui(f"❌ Активный бэкап в репо: {message}", ok=False)
            self._log(f"Smart preflight ABORT: {message}", "ERROR", channel="error")
            self.btn_bk.setEnabled(True)
            if prof.get("tg_fatal", self.cfg.tg_fatal):
                threading.Thread(target=tg_broadcast, args=(
                    self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group,
                    f"RestLex - Профиль: {self._pname()} | LockBlocked_ByActiveBackup: {message}"),
                    daemon=True).start()
            self._release_repo_mutex()
            return
        if code == 2:
            self._set_bk_result_status("error")
            self._bk_ui(f"⏸ Лок не stale, повтор позже: {message}", ok=False)
            self._log(f"Smart preflight WAIT: {message}", channel="info")
            self.btn_bk.setEnabled(True)
            self._release_repo_mutex()
            return
        # 99 — ошибка
        self._set_bk_result_status("error")
        self._bk_ui(f"❌ Smart preflight: {message}", ok=False)
        self._log(f"Smart preflight error: {message}", "ERROR", channel="error")
        self.btn_bk.setEnabled(True)
        if prof.get("tg_fatal", self.cfg.tg_fatal):
            threading.Thread(target=tg_broadcast, args=(
                self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group,
                f"RestLex - Профиль: {self._pname()} | Ошибка: Unlock_Failed ({message})"),
                daemon=True).start()
        self._release_repo_mutex()

    def _on_pre_backup_unlock_done(self,code,_):
        if self.sender() is not self.worker:
            return
        if code!=0:
            self._set_bk_result_status("error")
            self._bk_ui(f"❌ Не удалось подготовить репозиторий (unlock, код {code})",ok=False)
            self.btn_bk.setEnabled(True)
            self._ref_logs()
            prof=self.cfg.profile(self.pid) or {}
            if prof.get("tg_fatal", self.cfg.tg_fatal):
                threading.Thread(target=tg_broadcast, args=(
                    self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group,
                    f"RestLex - Профиль: {self._pname()} | Ошибка: Unlock_Failed"), daemon=True).start()
            self._release_repo_mutex()
            return
        self._run_pre_backup_steps()

    def _cancel_backup(self):
        # Остановить ещё не завершённую проверку, иначе поздний done снова
        # запустит бэкап после отмены пользователем.
        lp = getattr(self, "_lock_preflight", None)
        if lp is not None:
            lp.cancel()
            self._lock_preflight = None
        # Берём воркер БЭКАПА, а не текущего профиля: если пользователь переключил
        # профиль, self.pid другой — restic не останавливался, но флаги и RepoMutex
        # сбрасывались, и следующий бэкап шёл ПАРАЛЛЕЛЬНО в тот же репозиторий.
        w = self._workers.get(self._backup_pid or self.pid)
        if w and w.proc.state() != QProcess.NotRunning:
            self._log("⏹ Бэкап отменён пользователем", channel="info")
            if not w.stop(wait_ms=500):
                # Пока restic жив, mutex остаётся у нас. Иначе другой запуск
                # пройдёт preflight параллельно с ещё работающим процессом.
                self._bk_ui("⏳ Ожидание завершения restic после отмены…", running=True)
                self.btn_bk.setEnabled(False)
                QTimer.singleShot(1000, self._cancel_backup)
                return
        self._backup_running = False
        self._backup_paused = False
        self._set_bk_result_status("idle")
        self._bk_ui("❌ Бэкап отменён")
        self.btn_bk.setEnabled(True)
        self._release_repo_mutex()
        self._refresh_global_bk_banner()
        if self._backup_timeout_timer and self._backup_timeout_timer.isActive():
            self._backup_timeout_timer.stop()

    def _toggle_pause_backup(self):
        w = self._workers.get(self._backup_pid or self.pid)
        if not w or w.proc.state() == QProcess.NotRunning:
            return
        if self._backup_paused:
            if w.resume():
                self._backup_paused = False
                self.btn_bk_pause.setText("⏸ ПАУЗА")
                self.btn_bk_pause.setStyleSheet("font-weight:bold;color:#444;background:#fff8dd;border:1px solid #ddcc88;")
                self._log("▶ Бэкап возобновлён", channel="info")
            else:
                self._log("⚠ Не удалось возобновить бэкап", channel="error")
        else:
            if w.suspend():
                self._backup_paused = True
                self.btn_bk_pause.setText("▶ ПРОДОЛЖИТЬ")
                self.btn_bk_pause.setStyleSheet("font-weight:bold;color:#228822;background:#ddffdd;border:1px solid #88cc88;")
                self._log("⏸ Бэкап приостановлен", channel="info")
            else:
                self._log("⚠ Не удалось приостановить бэкап", channel="error")

    def _start_res_monitor(self):
        if self._res_monitor_timer:
            self._res_monitor_timer.stop()
        self._res_monitor_timer = QTimer(self)
        self._res_monitor_timer.setInterval(5000)
        self._res_monitor_timer.timeout.connect(self._update_res_status)
        self._res_monitor_timer.start()

    def _stop_res_monitor(self):
        if self._res_monitor_timer:
            self._res_monitor_timer.stop()

    def _update_res_status(self):
        if not self._backup_running:
            return
        threading.Thread(target=self._update_res_status_bg, daemon=True).start()

    def _get_ram_pct(self) -> int:
        """RAM % через Windows API GlobalMemoryStatusEx — без subprocess."""
        try:
            import ctypes
            class _MEMSTATEX(ctypes.Structure):
                _fields_ = [
                    ("dwLength",                ctypes.c_ulong),
                    ("dwMemoryLoad",            ctypes.c_ulong),
                    ("ullTotalPhys",            ctypes.c_ulonglong),
                    ("ullAvailPhys",            ctypes.c_ulonglong),
                    ("ullTotalPageFile",        ctypes.c_ulonglong),
                    ("ullAvailPageFile",        ctypes.c_ulonglong),
                    ("ullTotalVirtual",         ctypes.c_ulonglong),
                    ("ullAvailVirtual",         ctypes.c_ulonglong),
                    ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
                ]
            stat = _MEMSTATEX()
            stat.dwLength = ctypes.sizeof(_MEMSTATEX)
            ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(stat))
            return int(stat.dwMemoryLoad)
        except Exception:
            return 0

    def _get_cpu_pct(self) -> str:
        """CPU % — wmic первым, PowerShell Get-CimInstance как fallback (Win11 без wmic)."""
        try:
            r = subprocess.run(
                ["wmic", "cpu", "get", "loadpercentage", "/value"],
                capture_output=True, text=True, timeout=3,
                **hidden_console_subprocess_kwargs()
            )
            for line in (r.stdout or "").splitlines():
                if "LoadPercentage" in line:
                    val = line.split("=")[-1].strip()
                    if val.isdigit():
                        return val
        except Exception:
            pass
        try:
            r = subprocess.run(
                ["powershell", "-NoProfile", "-NonInteractive", "-Command",
                 "(Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average"],
                capture_output=True, text=True, timeout=5,
                **hidden_console_subprocess_kwargs()
            )
            val = (r.stdout or "").strip().split(".")[0]
            if val.lstrip("-").isdigit():
                return val
        except Exception:
            pass
        return "—"

    def _update_res_status_bg(self):
        try:
            if sys.platform != "win32":
                return
            cpu = self._get_cpu_pct()
            mem_pct = self._get_ram_pct()
            msg = f"⏳ Бэкап… CPU {cpu}% | RAM {mem_pct}%"
            self._ui_call_signal.emit(lambda: self.statusBar().showMessage(msg, 5000))
        except Exception:
            pass

    def _check_backup_timeout(self):
        if not self._backup_running or not self.pid:
            return
        timeout_hours = self.cfg.global_backup_timeout_hours
        if timeout_hours <= 0 or not self._bk_started_at:
            return
        elapsed = (datetime.now() - self._bk_started_at).total_seconds()
        if elapsed > timeout_hours * 3600:
            self._log(f"Бэкап длится дольше {timeout_hours} ч — отправляю уведомление", channel="info")
            tg_token = self.cfg.tg_token
            tg_chat = self.cfg.tg_chat
            if tg_token and tg_chat:
                threading.Thread(target=tg_broadcast,args=(tg_token,tg_chat,self.cfg.tg_group,
                    f"{self._pname()} - Таймаут: бэкап длится >{timeout_hours} ч"),daemon=True).start()
            # Don't repeat: stop timer
            if self._backup_timeout_timer:
                self._backup_timeout_timer.stop()

    def _run_backup(self):
        if not self.pid: return
        err=self._validate_req()
        if err:
            QMessageBox.warning(self,"Обязательные поля","Перейдите на вкладку «Репозиторий»:\n\n"+err)
            self.tabs.setCurrentWidget(self._tab_repo); return
        self._autosave(); self._save_global()
        prof=self.cfg.profile(self.pid) or {}
        repo=prof.get("repo_path","")
        if not repo.startswith(("sftp:","s3:","b2:","azure:","gs:","rclone:","rest:")):
            p=Path(repo)
            if not p.exists():
                try: p.mkdir(parents=True, exist_ok=True)
                except Exception as e:
                    QMessageBox.critical(self,"Ошибка",f"Не удалось создать папку:\n{repo}\n\n{e}"); return
            if p.exists() and not (p/"config").exists():
                if QMessageBox.question(self,"Репозиторий не найден",
                    f"Репозиторий не существует:\n{repo}\n\nСоздать новый?") == QMessageBox.Yes:
                    self._do_init_then_bk(); return
                else: return
        self._start_backup_preflight()

    def _do_init_then_bk(self):
        w=self._mk_worker()
        if not w: return
        w.out.connect(lambda m:self._log(m, channel="info"))
        w.err.connect(lambda m:self._log("❌ "+m,"ERROR", channel="info"))
        w.done.connect(lambda c,_:self._start_backup_preflight() if c==0 else
                       self._log(f"Инициализация не удалась (код {c})","ERROR", channel="info"))
        w.init(); self._workers[self.pid]=w

    def _do_bk(self):
        prof=self.cfg.profile(self.pid) or {}
        self._bk_started_at=datetime.now()
        self._backup_running = True
        self._backup_pid = self.pid
        self._bk_ui("⏳ Бэкап выполняется…",running=True)
        self._refresh_global_bk_banner()
        w=self._mk_worker()
        if not w:
            self._backup_running = False
            self._set_bk_result_status("idle")
            self._bk_ui("Готов")
            self.btn_bk.setEnabled(True)
            self._release_repo_mutex()
            return
        self._workers[self.pid]=w
        self._log("=== БЭКАП ===", channel="info")
        self._log(f"VSS shadow copy: {'ENABLED' if prof.get('use_vss', True) else 'disabled'}", channel="info")
        # Layer 2: heartbeat файл, видим в ProgramData\RestLex\active-backups
        try:
            if self._heartbeat:
                self._heartbeat.stop()
            self._heartbeat = BackupHeartbeat(prof.get("repo_path", "") or "", self._pname())
            self._heartbeat.start()
        except Exception:
            pass
        w.out.connect(self._on_backup_output)
        w.err.connect(self._on_backup_error)
        w.done.connect(self._on_bk_done)
        w.backup(
            prof.get("sources",[]),
            prof.get("use_vss",True),
            excl=prof.get("exclude",[]),
            no_scan=self.cfg.global_no_scan,
            vss_timeout_s=self.cfg.global_vss_timeout_s,
        )

    def _count_restic_processes(self) -> int:
        """Сколько restic.exe процессов сейчас запущено."""
        try:
            r = subprocess.run(
                ["tasklist", "/fi", "imagename eq restic.exe", "/fo", "csv", "/nh"],
                capture_output=True, text=True, timeout=3,
                **hidden_console_subprocess_kwargs()
            )
            return sum(1 for ln in r.stdout.splitlines() if "restic.exe" in ln.lower())
        except Exception:
            return 1  # не можем проверить — считаем что мы одни

    def _vss_reset_and_retry(self):
        """Перезапускаем VSS-службу и повторяем бэкап (только один раз)."""
        try:
            kw = hidden_console_subprocess_kwargs()
            self._log("⚠ Останавливаю службу VSS...", channel="info")
            subprocess.run(["net", "stop", "vss", "/y"], capture_output=True, timeout=15, **kw)
            time.sleep(3)
            self._log("⚠ Запускаю службу VSS...", channel="info")
            subprocess.run(["net", "start", "vss"], capture_output=True, timeout=15, **kw)
            time.sleep(2)
        except Exception as e:
            pass
        self._log("🔄 Повтор бэкапа после сброса VSS", channel="info")
        # Фоновый поток — singleShot тут не сработает (см. _vss_fix_and_continue).
        self._ui_call_signal.emit(self._start_backup_preflight)

    def _finish_background_backup(self, pid: str, code: int):
        """Завершение бэкапа профиля, который сейчас НЕ выбран в списке.
        UI-цепочка (retention/check) привязана к self.pid и продолжаться не
        может, но бэкап обязан завершиться по существу: флаги, RepoMutex,
        heartbeat, web-отчёт, Telegram. Иначе mutex висит навсегда.
        ponytail: retention/check фонового профиля пропускаются; полный фикс —
        прокинуть pid через всю цепочку бэкапа."""
        self._backup_running = False
        self._backup_pid = ""
        self._post_backup_code = code
        prof = self.cfg.profile(pid) or {}
        pname = prof.get("name", pid)
        self._log(f"Бэкап профиля «{pname}» завершён с кодом {code} "
                  f"(профиль не выбран — retention/check пропущены)", channel="info")
        threading.Thread(target=self._send_backup_web_report,
                         args=(code, pid), daemon=True).start()
        if code == 1 and prof.get("tg_fatal", self.cfg.tg_fatal):
            threading.Thread(target=tg_broadcast, args=(self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group,
                f"RestLex - Профиль: {pname} | Ошибка: Backup_Failed"), daemon=True).start()
        if code == 3 and prof.get("tg_file", self.cfg.tg_file):
            threading.Thread(target=tg_broadcast, args=(self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group,
                f"RestLex - Профиль: {pname} | Ошибка: Backup_Error (файлы)"), daemon=True).start()
        self._stop_res_monitor()
        self._release_repo_mutex()
        self._refresh_global_bk_banner()
        if self._backup_timeout_timer and self._backup_timeout_timer.isActive():
            self._backup_timeout_timer.stop()

    def _abort_background_chain(self):
        """Шаг цепочки бэкапа (pre-steps/clean/check) завершился, когда выбран
        другой профиль. Останавливаем цепочку, но не бросаем mutex и флаги."""
        bpid = self._backup_pid
        if bpid and bpid != self.pid and self.sender() is self._workers.get(bpid):
            self._log(f"⚠ Профиль переключён во время цепочки бэкапа ({bpid}) — "
                      f"цепочка остановлена, mutex освобождён", channel="info")
            self._backup_running = False
            self._backup_pid = ""
            self._stop_res_monitor()
            self._release_repo_mutex()
            self._refresh_global_bk_banner()
            if self._backup_timeout_timer and self._backup_timeout_timer.isActive():
                self._backup_timeout_timer.stop()

    def _on_bk_done(self,code,_):
        if self.sender() is not self.worker:
            bpid = self._backup_pid
            if bpid and self.sender() is self._workers.get(bpid):
                self._finish_background_backup(bpid, code)
            return
        self._snap_source_pid=None
        self._backup_running = False
        self._post_backup_code = code
        self._log(f"Бэкап завершён с кодом {code}", channel="info")
        # Auto-fix VSS_E_SNAPSHOT_SET_IN_PROGRESS: перезапустить VSS и повторить (1 раз)
        if code != 0 and getattr(self, "_bk_vss_stuck", False) and not getattr(self, "_bk_vss_retry_done", False):
            self._bk_vss_stuck = False
            self._bk_vss_retry_done = True
            self._log("⚠ VSS_E_SNAPSHOT_SET_IN_PROGRESS — авто-сброс VSS и повтор...", channel="info")
            self._bk_ui("⏳ Сброс VSS...", running=True)
            # H3-fix: останавливаем старый heartbeat до перезапуска бэкапа,
            # иначе finally старого треда удалит свежий heartbeat-файл.
            try:
                if self._heartbeat is not None:
                    self._heartbeat.stop()
                    self._heartbeat = None
            except Exception:
                pass
            threading.Thread(target=self._vss_reset_and_retry, daemon=True).start()
            return
        # pid передаём явно: внутри потока self.pid мог уже смениться (пользователь
        # переключил профиль) и отчёт уходил с чужим именем и repo.
        threading.Thread(target=self._send_backup_web_report,
                         args=(code, self.pid), daemon=True).start()
        self._set_bk_result_status("ok" if code == 0 else "error")
        prof=self.cfg.profile(self.pid) or {}
        tg_fatal=prof.get("tg_fatal", self.cfg.tg_fatal)
        tg_file=prof.get("tg_file", self.cfg.tg_file)
        if code==1 and tg_fatal:
            threading.Thread(target=tg_broadcast,args=(self.cfg.tg_token,self.cfg.tg_chat,self.cfg.tg_group,
                f"RestLex - Профиль: {self._pname()} | Ошибка: Backup_Failed"),daemon=True).start()
        if code==3 and tg_file:
            threading.Thread(target=tg_broadcast,args=(self.cfg.tg_token,self.cfg.tg_chat,self.cfg.tg_group,
                f"RestLex - Профиль: {self._pname()} | Ошибка: Backup_Error (файлы)"),daemon=True).start()
        prof=self.cfg.profile(self.pid) or {}
        ret=prof.get("retention",{})
        cleanup_after_backup = code in (0,3) and any(ret.values())
        if code in (0, 3) and not cleanup_after_backup:
            self._refresh_snaps(show_loading=False)
        if cleanup_after_backup:
            self._bk_ui("⏳ Подготавливаю применение правил хранения…",running=True)
            self._log("Автоматически применяю правила хранения после бэкапа", channel="info")
            self._log("Проверяю stale lock перед очисткой", channel="info")
            QTimer.singleShot(0, lambda: self._run_clean(from_backup=True))
            return
        self._run_post_backup_check_if_needed(from_backup=True)

    def _run_post_backup_check_if_needed(self, from_backup=False):
        if not self.cfg.global_post_backup_check:
            self._finish_backup_ui()
            return
        self._bk_ui("⏳ Проверяю репозиторий (check)…",running=True)
        # forget --prune может оставить orphan-лок на несколько сот мс —
        # снимаем его перед check, иначе exclusive lock падает с rc=11.
        self._log("=== CHECK ===", channel="info")
        u=self._mk_worker()
        if not u:
            self._finish_backup_ui()
            return
        self._workers[self.pid]=u
        u.out.connect(lambda m:self._log(m, channel="info"))
        u.err.connect(lambda m:self._log("❌ "+m,"ERROR", channel="error"))
        u.done.connect(lambda _c,_o,fb=from_backup:self._after_pre_check_unlock(fb))
        u.unlock()

    def _after_pre_check_unlock(self, from_backup=False):
        if self.sender() is not self.worker:
            self._abort_background_chain()
            return
        w=self._mk_worker()
        if not w:
            self._finish_backup_ui()
            return
        self._workers[self.pid]=w
        w.out.connect(lambda m:self._log(m, channel="info"))
        w.err.connect(lambda m:self._log("❌ "+m,"ERROR", channel="error"))
        w.done.connect(lambda c,_:self._on_post_check_done(c, from_backup))
        self._check_unknown_flag = False
        if self.cfg.global_check_host_only:
            w.err.connect(self._watch_check_unknown_flag)
            w.out.connect(self._watch_check_unknown_flag)
        w.check_repo(host_only=self.cfg.global_check_host_only)

    def _watch_check_unknown_flag(self, line: str):
        low = (line or "").lower()
        if "unknown flag" in low or "unknown option" in low or "unknown shorthand" in low:
            self._check_unknown_flag = True

    def _on_post_check_done(self, code, from_backup=False):
        if self.sender() is not self.worker:
            self._abort_background_chain()
            return
        self._log(f"Check завершён с кодом {code}", channel="info")
        if code != 0 and self._check_unknown_flag:
            self._check_unknown_flag = False
            self.cfg.global_check_host_only = False
            if hasattr(self, "chk_check_host_only"):
                self.chk_check_host_only.setChecked(False)
            self._log("⚠ restic не понимает «check --host» (нужен 0.19.0+). "
                      "Галочка «--check только свой хост» снята, репозиторий НЕ проверен.",
                      "ERROR", channel="error")
            self._finish_backup_ui()
            return
        if code != 0:
            prof=self.cfg.profile(self.pid) or {}
            tg_prune=prof.get("tg_prune", self.cfg.tg_prune)
            if tg_prune:
                threading.Thread(target=tg_broadcast,args=(self.cfg.tg_token,self.cfg.tg_chat,self.cfg.tg_group,
                    f"RestLex - Профиль: {self._pname()} | Ошибка: Check_Failed"),daemon=True).start()
        self._finish_backup_ui()

    def _finish_backup_ui(self):
        code = self._post_backup_code if self._post_backup_code is not None else 0
        self._backup_pid = ""   # цепочка (clean/check) закончена — теперь можно
        self._backup_paused = False
        self.btn_bk.setEnabled(True)
        self._stop_res_monitor()
        self._release_repo_mutex()
        self._refresh_global_bk_banner()
        if self._backup_timeout_timer and self._backup_timeout_timer.isActive():
            self._backup_timeout_timer.stop()
        self._ref_logs()
        if code==0:
            self._bk_ui("✅ Бэкап выполнен успешно",ok=True)
            self.statusBar().showMessage("✅ Бэкап выполнен",5000)
        elif code==3:
            self._bk_ui("⚠️ Бэкап завершён с ошибками копирования файлов (код 3)",ok=False)
        else:
            self._bk_ui(f"❌ Бэкап завершён с ошибкой (код {code})",ok=False)
        if code in (0, 3) and self.cfg.cache_cleanup:
            restic = self.cfg.restic
            # hidden_console_subprocess_kwargs — иначе в windowed-exe после каждого
            # бэкапа мигает чёрное консольное окно; timeout — чтобы поток не завис.
            def _cache_cleanup():
                try:
                    subprocess.run([restic, "cache", "--cleanup"], capture_output=True,
                                   timeout=300, **hidden_console_subprocess_kwargs())
                except Exception:
                    pass
            threading.Thread(target=_cache_cleanup, daemon=True).start()
        if code == 0:
            self._maybe_shutdown_after_backup()
        # code != 0 уже покрыт Telegram-уведомлением в _on_bk_done (Backup_Failed / Backup_Error)

    def _tg_send(self, text: str):
        threading.Thread(target=tg_broadcast, args=(
            self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group, text), daemon=True).start()

    def _maybe_shutdown_after_backup(self):
        """Выключение ПК после успешного бэкапа. Два контроля: код возврата
        shutdown.exe (нет прав / уже запланировано) и watchdog-поток — если через
        N+запас секунд процесс жив, ОС не выключилась (shutdown /a, блокирующий
        диалог, Windows Update)."""
        prof = self.cfg.profile(self.pid) or {}
        delay = max(0, int(prof.get("shutdown_delay", SHUTDOWN_DELAY_DEF) or 0))
        pname = self._pname()
        if prof.get("shutdown_remote", False):
            mac = str(prof.get("shutdown_remote_mac", "") or "")
            def _remote():
                target = resolve_power_target(mac, prof)
                if not send_sleep(mac, target):
                    self._ui_call_signal.emit(lambda: self._log(
                        f"❌ Пакет выключения на {mac} не отправлен (MAC некорректен?)", "ERROR", channel="error"))
                    self._tg_send(f"RestLex - Профиль: {pname} | Ошибка: пакет выключения {mac} не отправлен")
                    return
                self._ui_call_signal.emit(lambda: self._log(
                    f"Sleep-on-LAN: UDP-пакет отправлен для MAC {mac}; "
                    f"доставка и выполнение выключения не подтверждены", channel="info"))
                if target:
                    time.sleep(SLEEP_ON_LAN_CHECK)
                    reachable = self._host_reachable(target, 3)
                    observation = sleep_on_lan_probe_message(pname, mac, target, reachable)
                    self._ui_call_signal.emit(lambda msg=observation: self._log(msg, channel="info"))
                    if reachable:
                        self._tg_send(observation.replace(" | Sleep-on-LAN:", " | Предупреждение Sleep-on-LAN:", 1))
                else:
                    self._ui_call_signal.emit(lambda: self._log(
                        f"Sleep-on-LAN: IP для MAC {mac} неизвестен; ping-проверка пропущена",
                        channel="info"))
            threading.Thread(target=_remote, daemon=True).start()
        if not prof.get("shutdown_after_backup", False):
            return
        self._log(f"Выключение ПК через {delay} сек (shutdown /s /t {delay})", channel="info")
        ev = threading.Event()
        self._shutdown_watch_ev = ev
        def _run():
            # shutdown.exe может отвечать секундами (pending update, GPO) — только
            # в фоне, UI-поток не блокируем; в виджеты пишем через _ui_call_signal.
            try:
                kw = {"capture_output": True, "text": True, "encoding": "oem", "errors": "replace", "timeout": 30}
                kw.update(hidden_console_subprocess_kwargs())
                r = subprocess.run(["shutdown", "/s", "/t", str(delay)], **kw)
                rc, detail = r.returncode, ((r.stderr or "") + (r.stdout or "")).strip()
            except Exception as e:
                rc, detail = -1, str(e)
            if rc != 0:
                self._ui_call_signal.emit(lambda: (
                    self._log(f"❌ Не удалось запланировать выключение (код {rc}): {detail}",
                              "ERROR", channel="error"),
                    self._bk_ui("⚠️ Бэкап успешен, но выключение ПК не запланировано", ok=False)))
                self._tg_send(f"RestLex - Профиль: {pname} | Ошибка: выключение ПК не запланировано "
                              f"(код {rc}) {detail[:200]}")
                return
            self._ui_call_signal.emit(
                lambda: self._bk_ui(f"✅ Бэкап выполнен, выключение через {delay} сек", ok=True))
            # Если ОС выключилась вовремя — процесс убит и до отправки не дойдём.
            # Если пользователь закрыл GUI — _shutdown_runtime выставит event.
            if ev.wait(delay + SHUTDOWN_CHECK_MARGIN):
                return
            tg_broadcast(self.cfg.tg_token, self.cfg.tg_chat, self.cfg.tg_group,
                         f"RestLex - Профиль: {pname} | Ошибка: выключение ПК не состоялось "
                         f"(прошло {delay + SHUTDOWN_CHECK_MARGIN} сек после команды)")
        threading.Thread(target=_run, daemon=True).start()

    def _run_clean(self, from_backup=False):
        if not self.pid: return
        if from_backup:
            self._bk_ui("⏳ Проверяю lock репозитория перед очисткой…",running=True)
        else:
            self._log("Проверяю stale lock перед очисткой", channel="info")
        w=self._mk_worker()
        if not w:
            if from_backup:
                self.btn_bk.setEnabled(True)
                code = self._post_backup_code if self._post_backup_code is not None else 0
                if code==0:
                    self._bk_ui("✅ Бэкап выполнен успешно",ok=True)
                elif code==3:
                    self._bk_ui("⚠️ Бэкап завершён с ошибками копирования файлов (код 3)",ok=False)
            return
        self._workers[self.pid]=w
        w.out.connect(lambda m:self._log(m, channel="info"))
        w.err.connect(lambda m:self._log("❌ "+m,"ERROR", channel="error"))
        w.done.connect(lambda c,_,fb=from_backup:self._on_pre_clean_unlock_done(c, fb))
        w.unlock()

    def _on_pre_clean_unlock_done(self,code,from_backup=False):
        if self.sender() is not self.worker:
            self._abort_background_chain()
            return
        if code!=0:
            self.btn_bk.setEnabled(True)
            self._ref_logs()
            if from_backup:
                self._bk_ui(f"⚠️ Очистка пропущена: unlock завершился с ошибкой (код {code})",ok=False)
            else:
                self._bk_ui("Готов")
                QMessageBox.warning(self,"Очистка",f"Не удалось подготовить репозиторий (unlock).\nКод: {code}")
            return
        self._run_clean_impl(from_backup)

    def _run_clean_impl(self, from_backup=False):
        if not self.pid: return
        prof=self.cfg.profile(self.pid) or {}
        ret=prof.get("retention",{})
        if not any(ret.values()):
            self._log("Правила хранения не заданы — очистка пропущена", channel="info")
            if from_backup:
                self._run_post_backup_check_if_needed(from_backup=True)
            else:
                self._bk_ui("Готов")
                self.btn_bk.setEnabled(True)
            return
        self._log("=== ОЧИСТКА (forget/prune) ===", channel="info")
        w=self._mk_worker()
        if not w:
            if from_backup:
                self.btn_bk.setEnabled(True)
                code = self._post_backup_code if self._post_backup_code is not None else 0
                if code==0:
                    self._bk_ui("✅ Бэкап выполнен успешно",ok=True)
                elif code==3:
                    self._bk_ui("⚠️ Бэкап завершён с ошибками копирования файлов (код 3)",ok=False)
            return
        self._workers[self.pid]=w
        w.out.connect(lambda m:self._log(m, channel="info"))
        w.err.connect(lambda m:self._log("❌ "+m,"ERROR", channel="error"))
        w.done.connect(lambda c,_,fb=from_backup:self._on_clean_done(c, fb)); w.forget_pol(ret)

    def _on_clean_done(self,code,from_backup=False):
        if self.sender() is not self.worker:
            self._abort_background_chain()
            return
        self._snap_source_pid=None
        self._log(f"Очистка завершена с кодом {code}", channel="info")
        self._ref_logs()
        backup_code = self._post_backup_code if self._post_backup_code is not None else 0
        if from_backup and backup_code in (0,3) and self.pid:
            self._refresh_snaps(show_loading=False)
        elif code == 0 and self.pid:
            self._refresh_snaps(show_loading=self._is_restore_tab_active())
        if from_backup:
            if code != 0:
                prof=self.cfg.profile(self.pid) or {}
                tg_prune=prof.get("tg_prune", self.cfg.tg_prune)
                if tg_prune:
                    threading.Thread(target=tg_broadcast,args=(self.cfg.tg_token,self.cfg.tg_chat,self.cfg.tg_group,
                        f"RestLex - Профиль: {self._pname()} | Ошибка: Prune_Failed"),daemon=True).start()
            self._run_post_backup_check_if_needed(from_backup=True)
            return
        self.btn_bk.setEnabled(True)
        self._bk_ui("Готов")
        if code == 0:
            self.statusBar().showMessage("✅ Очистка завершена",5000)
            QMessageBox.information(self,"Очистка","Очистка завершена.")
        else:
            QMessageBox.warning(self,"Очистка",f"Очистка завершилась с ошибкой.\nКод: {code}")
        prof=self.cfg.profile(self.pid) or {}
        tg_prune=prof.get("tg_prune", self.cfg.tg_prune)
        if code in(1,3) and tg_prune:
            threading.Thread(target=tg_broadcast,args=(self.cfg.tg_token,self.cfg.tg_chat,self.cfg.tg_group,
                f"RestLex - Профиль: {self._pname()} | Ошибка: Prune_Failed"),daemon=True).start()

    def _run_clean_manual(self):
        if not self.pid:
            QMessageBox.warning(self,"Очистка","Выберите профиль"); return
        if QMessageBox.question(
            self,
            "Очистка",
            "Запустить очистку по текущим правилам хранения?",
        ) != QMessageBox.Yes:
            return
        self._run_clean(from_backup=False)

    # ─── RESTORE ─────────────────────────────────────────────────────────────

    def _selected_snap_paths(self, include_current_dir: bool = True) -> Optional[List[str]]:
        dsel = self._deleted_pane_selection()
        if dsel:
            return compact_snap_paths(list(dsel))
        if getattr(self, "_snap_view_mode", "tree") == "search":
            items = self.snap_search_list.selectedItems()
            paths = [it.data(Qt.UserRole) for it in items if it.data(Qt.UserRole)]
            return compact_snap_paths(paths) if paths else None
        if self._using_snap_browser():
            files = self._browser_selected_snap_paths()
            if not files and include_current_dir:
                cur_dir = self._browser_current_snap_path()
                if cur_dir and cur_dir != "/":
                    files = [cur_dir]
                else:
                    files = None
            return files
        sel=self.snap_tree.selectedItems()
        return compact_snap_paths([it.data(0,Qt.UserRole) for it in sel]) if sel else None

    def _documents_dir(self) -> Path:
        docs = Path.home() / "Documents"
        if docs.exists():
            return docs
        return Path.home()

    def _set_restore_buttons_enabled(self, enabled: bool):
        for btn in (
            getattr(self, "btn_restore_inplace", None),
            getattr(self, "btn_restore_docs", None),
            getattr(self, "btn_snap_open", None),
            getattr(self, "btn_snap_del", None),
            getattr(self, "btn_snap_ref", None),
            getattr(self, "snap_combo", None),
        ):
            if btn:
                btn.setEnabled(enabled)

    def _legacy_nested_restore_path(self, local_path: Path) -> Optional[Path]:
        target = Path(local_path)
        parts = list(target.parts)
        if len(parts) < 2:
            return None
        anchor = str(parts[0] or "")
        drive = anchor.rstrip("\\/").rstrip(":")
        rel_parts = [part for part in parts[1:] if part not in ("\\", "/")]
        if not drive or not rel_parts:
            return None
        nested = target / drive / Path(*rel_parts)
        try:
            nested.relative_to(target)
        except Exception:
            return None
        return nested

    def _cleanup_legacy_inplace_dir(self, dest_path: Path):
        nested = self._legacy_nested_restore_path(dest_path)
        if not nested or not nested.exists():
            return
        try:
            if nested.is_dir():
                shutil.rmtree(str(nested))
            else:
                nested.unlink()
            msg = f"Удалён старый ошибочный путь: {nested}"
            self._append_restore_progress(msg, current=str(dest_path))
            self._log(msg, channel="info")
        except Exception as e:
            self._log(f"⚠ Не удалось удалить старый ошибочный путь {nested}: {e}", "ERROR", channel="error")

    def _restore_snapshot_spec(self, snap_root: str) -> str:
        snap_root = normalize_snap_path(snap_root)
        if not self._cur_snap_id:
            raise RuntimeError("Не выбран снапшот")
        if snap_root == "/":
            return self._cur_snap_id
        return f"{self._cur_snap_id}:{snap_root}"

    def _selected_snap_entries(self) -> Optional[List[dict]]:
        dsel = self._deleted_pane_selection()
        if dsel:
            return [{"path": p, "type": dsel.get(p, "unknown")} for p in compact_snap_paths(list(dsel))]
        if getattr(self, "_snap_view_mode", "tree") == "search":
            kinds = {normalize_snap_path(it.data(Qt.UserRole)): str(it.data(Qt.UserRole + 1) or "unknown")
                     for it in self.snap_search_list.selectedItems() if it.data(Qt.UserRole)}
            return [{"path": p, "type": kinds.get(p, "unknown")} for p in compact_snap_paths(list(kinds))] or None
        if self._using_snap_browser():
            files = self._browser_selected_snap_paths()
            if not files:
                return None
            path_map = {normalize_snap_path(p): "unknown" for p in files if p}
        else:
            sel = self.snap_tree.selectedItems()
            if not sel:
                return None
            path_map = {}
            for it in sel:
                path = normalize_snap_path(it.data(0, Qt.UserRole) or "")
                if path and path != "/":
                    path_map[path] = str(it.data(0, Qt.UserRole + 1) or "unknown")
        if not path_map:
            return None
        return [{"path": path, "type": path_map.get(path, "unknown")} for path in compact_snap_paths(list(path_map.keys()))]

    def _require_selected_restore_paths(self) -> Optional[List[str]]:
        if not self._cur_snap_id:
            QMessageBox.warning(self, "Восстановление", "Выберите снапшот")
            return None
        entries = self._selected_snap_entries()
        if not entries:
            QMessageBox.warning(self, "Восстановление", "Выберите данные для восстановления")
            return None
        return [entry["path"] for entry in entries]

    def _require_selected_restore_entries(self) -> Optional[List[dict]]:
        if not self._cur_snap_id:
            QMessageBox.warning(self, "Восстановление", "Выберите снапшот")
            return None
        entries = self._selected_snap_entries()
        if not entries:
            QMessageBox.warning(self, "Восстановление", "Выберите данные для восстановления")
            return None
        return entries

    def _snap_path_to_windows_path(self, snap_path: str) -> Optional[Path]:
        pure = PurePosixPath(normalize_snap_path(snap_path))
        parts = [p for p in pure.parts if p not in ("/", "")]
        if not parts:
            return None
        common_roots = {
            "users", "windows", "programdata", "program files",
            "program files (x86)", "documents and settings", "$recycle.bin",
            "recovery", "inetpub",
        }
        best = None
        for idx, part in enumerate(parts):
            if not re.match(r"^[A-Za-z](?::)?$", part):
                continue
            drive = part[0].upper() + ":"
            rest = parts[idx + 1:]
            candidate = Path(drive + "\\")
            if rest:
                candidate = Path(drive + "\\", *rest)
            score = 0
            score += 2 if idx == 0 else 1
            if len(part) == 2 and part.endswith(":"):
                score += 1
            drive_root = Path(drive + "\\")
            if drive_root.exists():
                score += 4
            if rest and rest[0].lower() in common_roots:
                score += 5
            if candidate.exists():
                score += 4
            elif candidate.parent.exists():
                score += 2
            item = (score, idx, candidate)
            if best is None or (item[0], item[1]) > (best[0], best[1]):
                best = item
        if best is not None:
            return best[2]
        if parts[0].lower() in common_roots:
            drive = Path.home().drive or "C:"
            return Path(drive + "\\", *parts)
        return None

    def _default_restore_target_for_paths(self, paths: Optional[List[str]]) -> Path:
        docs = self._documents_dir()
        if not paths:
            return docs
        for snap_path in paths:
            candidate = self._snap_path_to_windows_path(snap_path)
            if candidate and candidate.exists():
                return candidate.parent if candidate.is_file() else candidate
        return docs

    def _unique_restore_destination(self, base: Path, used: set) -> Path:
        candidate = base
        idx = 2
        key = str(candidate).lower()
        while key in used:
            if base.suffix:
                candidate = base.with_name(f"{base.stem} ({idx}){base.suffix}")
            else:
                candidate = base.with_name(f"{base.name} ({idx})")
            idx += 1
            key = str(candidate).lower()
        used.add(key)
        return candidate

    def _documents_restore_destinations(self, paths: List[str], target_root: Optional[Path] = None) -> Dict[str, Path]:
        docs = target_root or self._documents_dir()
        used = set()
        out: Dict[str, Path] = {}
        for snap_path in paths:
            pure = PurePosixPath(normalize_snap_path(snap_path))
            name = pure.name or "восстановлено"
            out[snap_path] = self._unique_restore_destination(docs / name, used)
        return out

    def _inplace_restore_destinations(self, paths: List[str]) -> Optional[Dict[str, Path]]:
        out: Dict[str, Path] = {}
        unresolved: List[str] = []
        for snap_path in paths:
            target = self._snap_path_to_windows_path(snap_path)
            if not target:
                unresolved.append(snap_path)
                continue
            out[snap_path] = target
        if not unresolved:
            return out
        preview = "\n".join(unresolved[:5])
        if len(unresolved) > 5:
            preview += f"\n… и ещё {len(unresolved) - 5}"
        QMessageBox.warning(
            self,
            "Восстановление",
            "Не удалось определить исходный путь для восстановления:\n\n" + preview,
        )
        return None

    def _restic_cli_config(self) -> Optional[dict]:
        worker = self._mk_worker()
        if not worker:
            return None
        env = os.environ.copy()
        env["RESTIC_REPOSITORY"] = worker.repo
        env.setdefault("LOCALAPPDATA", os.environ.get("LOCALAPPDATA", str(Path.home() / "AppData" / "Local")))
        if worker.password:
            env["RESTIC_PASSWORD"] = worker.password
        if worker.temp:
            for key in ("TMPDIR", "TEMP", "TMP"):
                env[key] = worker.temp
        for key, value in worker.extra.items():
            if value:
                env[key] = value
        return {"restic": worker.restic, "repo": worker.repo, "env": env}

    def _build_restore_requests(self, entries: List[dict], target_root: Optional[Path] = None, inplace: bool = False) -> List[dict]:
        requests = []
        for entry in entries:
            snap_path = normalize_snap_path(entry.get("path") or "")
            pure = PurePosixPath(snap_path)
            name = pure.name or snap_path.strip("/") or "root"
            item_type = str(entry.get("type") or "unknown")
            local_path = self._snap_path_to_windows_path(snap_path) if inplace else None
            if inplace and not local_path:
                raise RuntimeError(f"Не удалось определить исходный путь: {snap_path}")
            if snap_path == "/":
                raise RuntimeError("Восстановление корня снапшота не поддерживается")
            if item_type == "dir":
                snapshot_spec = self._restore_snapshot_spec(snap_path)
                include = ""
                if inplace:
                    dest_path = local_path
                    target_dir = local_path
                    if local_path == Path(local_path.anchor):
                        raise RuntimeError("Восстановление корня диска по месту не поддерживается")
                else:
                    dest_path = Path(target_root) / name
                    target_dir = dest_path
            else:
                parent_snap = normalize_snap_path(str(pure.parent))
                snapshot_spec = self._restore_snapshot_spec(parent_snap)
                include = "/" + name
                if inplace:
                    dest_path = local_path
                    target_dir = local_path.parent
                else:
                    dest_path = Path(target_root) / name
                    target_dir = Path(target_root)
            requests.append({
                "snap_path": snap_path,
                "type": item_type,
                "snapshot_spec": snapshot_spec,
                "include": include,
                "target_dir": Path(target_dir),
                "dest_path": Path(dest_path),
                "label": str(dest_path),
            })
        return requests

    def _run_restore_request(self, cfg: dict, request: dict) -> int:
        args = [
            cfg["restic"],
            "restore",
            request["snapshot_spec"],
            "--target",
            str(request["target_dir"]),
        ]
        if request["include"]:
            args.extend(["--include", request["include"]])
        self._append_restore_progress(
            f"{request['snapshot_spec']}{(' ' + request['include']) if request['include'] else ''} -> {request['target_dir']}",
            current=request["label"],
        )
        proc = subprocess.Popen(
            args,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=cfg["env"],
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            **hidden_console_subprocess_kwargs(),
        )
        try:
            stream = proc.stdout
            if stream is not None:
                for line in stream:
                    text = line.rstrip()
                    if text:
                        self._append_restore_progress(text, current=request["label"])
                        self._log(text, channel="info")
            return proc.wait()
        finally:
            if proc.stdout is not None:
                proc.stdout.close()

    def _run_restore_request_bg(self, cfg: dict, request: dict, q: "queue.Queue") -> int:
        """Thread-safe version: puts progress into queue instead of touching UI directly."""
        q.put(("log", f"{request['snapshot_spec']}{(' '+request['include']) if request['include'] else ''} -> {request['target_dir']}"))
        q.put(("current", request["label"]))
        args = [
            cfg["restic"], "restore",
            request["snapshot_spec"], "--target", str(request["target_dir"]),
        ]
        if request["include"]:
            args.extend(["--include", request["include"]])
        proc = subprocess.Popen(
            args,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            env=cfg["env"], encoding="utf-8", errors="replace", bufsize=1,
            **hidden_console_subprocess_kwargs(),
        )
        # Держим ссылку, чтобы _shutdown_runtime мог убить restic при закрытии окна.
        self._restore_proc = proc
        try:
            stream = proc.stdout
            if stream is not None:
                for line in stream:
                    text = line.rstrip()
                    if text:
                        q.put(("log", text))
                        q.put(("current", request["label"]))
            return proc.wait()
        finally:
            self._restore_proc = None
            if proc.stdout is not None:
                proc.stdout.close()

    def _restore_worker_thread(self, cfg: dict, requests: list, job: dict, q: "queue.Queue"):
        """Runs in daemon thread. Processes all restore requests and signals completion."""
        try:
            total = max(1, len(requests))
            done = 0
            errors = []
            for req in requests:
                if self._closing:   # окно закрыто — не запускаем следующий restic
                    q.put(("all_done", errors + ["восстановление прервано закрытием окна"], job))
                    return
                if job.get("mode") == "inplace" and req.get("type") == "dir":
                    self._cleanup_legacy_inplace_dir(Path(req["dest_path"]))
                q.put(("progress", done, total, req["label"]))
                code = self._run_restore_request_bg(cfg, req, q)
                if code != 0:
                    errors.append(f"{req['label']}: restic вернул код {code}")
                else:
                    done += 1
                    q.put(("progress", done, total, req["label"]))
            q.put(("all_done", errors, job))
        except Exception as e:
            q.put(("all_error", str(e)))

    def _poll_restore_queue(self):
        """Called by QTimer every 50 ms — drains the restore queue and updates UI."""
        if not hasattr(self, "_restore_q") or self._restore_q is None:
            return
        for _ in range(30):
            try:
                msg = self._restore_q.get_nowait()
            except Exception:
                break
            kind = msg[0]
            if kind == "log":
                if self._restore_progress_dialog is not None:
                    self._restore_progress_log.appendPlainText(msg[1])
                    if not self._restore_progress_dialog.isVisible():
                        self.statusBar().showMessage(f"⏳ Восстановление…")
            elif kind == "current":
                if self._restore_progress_dialog is not None:
                    self._restore_progress_file_lbl.setText(msg[1])
                self.statusBar().showMessage(f"⏳ Восстановление: {msg[1]}")
            elif kind == "progress":
                _, done, total, label = msg
                if self._restore_progress_dialog is not None:
                    self._restore_progress_bar.setRange(0, total)
                    self._restore_progress_bar.setValue(done)
                    self._restore_progress_status_lbl.setText("Восстанавливаю…")
            elif kind == "all_done":
                _, errors, job = msg
                self._finish_restore_after_thread(errors, job)
                return
            elif kind == "all_error":
                self._finish_restore_after_thread([msg[1]], {})
                return

    def _finish_restore_after_thread(self, errors: list, job: dict):
        """Runs on main thread after background restore completes."""
        if hasattr(self, "_restore_timer") and self._restore_timer is not None:
            self._restore_timer.stop()
            self._restore_timer = None
        self._restore_q = None
        self._restore_in_progress = False
        self._hide_restore_progress()
        self._replace_all = False
        self._set_restore_buttons_enabled(True)
        for line in errors:
            self._log(f"❌ {line}", "ERROR", channel="error")
        ok = not errors
        self._log(f"Восстановление завершено: код {0 if ok else 1}")
        if ok:
            reveal_path = str(job.get("reveal_path") or "").strip()
            if reveal_path:
                self._open_path_in_explorer(Path(reveal_path), select=bool(job.get("reveal_select")))
            self.statusBar().showMessage("✅ Восстановление завершено", 5000)
        else:
            QMessageBox.warning(self, "Восстановление", "Часть данных не удалось восстановить.\nПодробности записаны в лог.")
        QTimer.singleShot(400, self._refresh_local)

    def _restore_job_conflicts(self, destinations: Dict[str, Path]) -> Dict[str, str]:
        conflicts: Dict[str, str] = {}
        seen = set()
        for snap_path, out_path in destinations.items():
            key = str(Path(out_path)).lower()
            if key in seen or Path(out_path).exists():
                conflicts[snap_path] = str(out_path)
            seen.add(key)
        return conflicts

    def _open_path_in_explorer(self, path: Path, select: bool = False):
        target = Path(path)
        if os.name == "nt":
            if select and target.exists():
                try:
                    subprocess.Popen(["explorer.exe", "/select,", str(target)])
                    return
                except Exception:
                    pass
            folder = target if target.is_dir() else target.parent
            try:
                subprocess.Popen(["explorer.exe", str(folder)])
                return
            except Exception:
                if hasattr(os, "startfile"):
                    os.startfile(str(folder))
                    return
        if hasattr(os, "startfile"):
            os.startfile(str(target))
        else:
            subprocess.Popen(["xdg-open", str(target if target.is_dir() else target.parent)])

    def _snapshot_restore_root(self) -> Path:
        if self._closing:
            raise RuntimeError("приложение закрывается")
        unc_root = self._ensure_snap_browser_mount()
        self._mount_connect_session(unc_root)
        target = self._snap_browser_target()
        if not target:
            raise RuntimeError("Не удалось определить путь снапшота")
        return Path(target)

    def _snapshot_source_path(self, snapshot_root: Path, snap_path: str) -> Path:
        return restore_output_path(str(snapshot_root), snap_path)

    def _collect_restore_tasks(self, src: Path, dst: Path, tasks: List[dict]):
        if src.is_dir():
            tasks.append({"kind": "dir", "src": src, "dst": dst, "size": 0})
            try:
                entries = sorted(src.iterdir(), key=lambda p: (not p.is_dir(), p.name.lower()))
            except OSError as e:
                raise RuntimeError(f"{src}: {e}")
            for child in entries:
                self._collect_restore_tasks(child, dst / child.name, tasks)
            return
        try:
            size = max(0, int(src.stat().st_size))
        except OSError:
            size = 0
        tasks.append({"kind": "file", "src": src, "dst": dst, "size": size})

    def _copy_file_with_progress(self, src: Path, dst: Path, progress: dict):
        if dst.exists() and dst.is_dir():
            shutil.rmtree(str(dst))
        dst.parent.mkdir(parents=True, exist_ok=True)
        chunk_size = 1024 * 1024
        with src.open("rb") as rf, dst.open("wb") as wf:
            while True:
                chunk = rf.read(chunk_size)
                if not chunk:
                    break
                wf.write(chunk)
                progress["done_bytes"] += len(chunk)
                now = time.time()
                if now - progress["last_ui"] >= 0.05 or progress["done_bytes"] >= progress["total_bytes"]:
                    scaled_value = min(
                        progress["maximum_value"],
                        max(0, (progress["done_bytes"] + progress["scale"] - 1) // progress["scale"]),
                    )
                    self._set_restore_progress(
                        current=str(src),
                        maximum=progress["maximum_value"],
                        value=scaled_value,
                    )
                    progress["last_ui"] = now
        try:
            shutil.copystat(str(src), str(dst))
        except Exception:
            pass

    def _execute_restore_copy(self, job: dict, files: List[str]) -> List[str]:
        errors: List[str] = []
        snapshot_root = self._snapshot_restore_root()
        tasks: List[dict] = []
        destinations = {k: Path(v) for k, v in (job.get("destinations") or {}).items()}
        for snap_path in files:
            src = self._snapshot_source_path(snapshot_root, snap_path)
            dst = destinations.get(snap_path)
            if dst is None:
                continue
            if not src.exists():
                errors.append(f"{snap_path}: объект не найден в снапшоте")
                continue
            try:
                self._collect_restore_tasks(src, dst, tasks)
            except Exception as e:
                errors.append(str(e))
        total_bytes = sum(task["size"] for task in tasks if task["kind"] == "file")
        total_steps = max(1, len(tasks))
        scale = 1024 * 1024 if total_bytes > 0 else 1
        maximum_value = max(1, (total_bytes + scale - 1) // scale) if total_bytes > 0 else total_steps
        progress = {
            "done_bytes": 0,
            "total_bytes": max(1, total_bytes),
            "last_ui": 0.0,
            "scale": scale,
            "maximum_value": maximum_value,
        }
        self._show_restore_progress("Восстанавливаю данные…", maximum=maximum_value, value=0)
        for task in tasks:
            src = task["src"]
            dst = task["dst"]
            try:
                self._append_restore_progress(f"{src} -> {dst}", current=str(src))
                if task["kind"] == "dir":
                    if dst.exists() and dst.is_file():
                        dst.unlink()
                    dst.mkdir(parents=True, exist_ok=True)
                    if total_bytes == 0:
                        progress["done_bytes"] += 1
                        self._set_restore_progress(current=str(src), maximum=maximum_value, value=progress["done_bytes"])
                else:
                    self._copy_file_with_progress(src, dst, progress)
                    if total_bytes == 0:
                        progress["done_bytes"] += 1
                        self._set_restore_progress(current=str(src), maximum=maximum_value, value=progress["done_bytes"])
                self._log(f"restore copy: {src} -> {dst}", channel="debug")
            except Exception as e:
                errors.append(f"{dst}: {e}")
        if total_bytes > 0:
            self._set_restore_progress(maximum=maximum_value, value=maximum_value)
        reveal_path = str(job.get("reveal_path") or "").strip()
        if reveal_path and not errors:
            self._open_path_in_explorer(Path(reveal_path), select=bool(job.get("reveal_select")))
        return errors

    def _snap_ctx(self, pos: QPoint):
        item=self.snap_tree.itemAt(pos)
        if item and not item.isSelected():
            self.snap_tree.clearSelection()
            item.setSelected(True)
            self.snap_tree.setCurrentItem(item)
        menu=QMenu(self)
        a_restore_inplace=menu.addAction("📍 Восстановить по месту хранения")
        a_restore_inplace.triggered.connect(self._restore_selected_in_place)
        a_restore_docs=menu.addAction("📥 Восстановить выбранное")
        a_restore_docs.triggered.connect(self._restore_selected_to_documents)
        menu.exec_(QCursor.pos())

    def _snap_deleted_ctx(self, pos: QPoint):
        item = self.snap_deleted.itemAt(pos)
        if item and not item.isSelected():
            self.snap_deleted.clearSelection()
            item.setSelected(True)
        self._last_pick = "deleted"
        menu = QMenu(self)
        menu.addAction("📍 Восстановить по месту хранения").triggered.connect(self._restore_selected_in_place)
        menu.addAction("📂 Восстановить выбранное").triggered.connect(self._restore_selected_to_documents)
        def _copy_path():
            cur = self.snap_deleted.currentItem()
            if cur:
                QApplication.clipboard().setText(cur.data(0, Qt.UserRole) or "")
        menu.addAction("📋 Скопировать путь").triggered.connect(_copy_path)
        menu.exec_(QCursor.pos())

    def _deleted_pane_selection(self) -> Dict[str, str]:
        """Выбор в правом дереве (только настоящие «удалённые» строки) — если последний клик был там."""
        if getattr(self, "_last_pick", "tree") != "deleted":
            return {}
        return {normalize_snap_path(it.data(0, Qt.UserRole)): str(it.data(0, Qt.UserRole + 1) or "unknown")
                for it in self.snap_deleted.selectedItems()
                if it.data(0, Qt.UserRole) and it.data(0, Qt.UserRole + 6) == "deleted"}

    def _snap_search_ctx(self, pos: QPoint):
        item = self.snap_search_list.itemAt(pos)
        if item and not item.isSelected():
            self.snap_search_list.clearSelection()
            item.setSelected(True)
        menu = QMenu(self)
        a_inplace = menu.addAction("📍 Восстановить по месту хранения")
        a_inplace.triggered.connect(self._restore_selected_in_place)
        a_restore = menu.addAction("📂 Восстановить выбранное")
        a_restore.triggered.connect(self._restore_selected_to_documents)
        a_copy = menu.addAction("📋 Скопировать путь")
        def _copy_path():
            it = self.snap_search_list.currentItem()
            if it:
                QApplication.clipboard().setText(it.text())
        a_copy.triggered.connect(_copy_path)
        menu.exec_(QCursor.pos())

    def _prompt_restore_selected(self):
        self._restore_selected_to_documents()

    def _restore_selected_to_documents(self):
        entries = self._require_selected_restore_entries()
        if not entries:
            return
        target_dir = QFileDialog.getExistingDirectory(
            self,
            "Куда восстановить?",
            str(self._default_restore_target_for_paths([entry["path"] for entry in entries])),
        )
        if not target_dir:
            return
        target_root = Path(target_dir)
        requests = self._build_restore_requests(entries, target_root=target_root, inplace=False)
        reveal_target = requests[0]["dest_path"] if len(requests) == 1 else target_root
        job = {
            "mode": "target",
            "requests": requests,
            "display_target": str(target_root),
            "reveal_path": str(reveal_target),
            "reveal_select": len(requests) == 1,
        }
        self._run_restore(target_ov=str(target_root), files_ov=[entry["path"] for entry in entries], restore_job=job)

    def _restore_selected_in_place(self):
        entries = self._require_selected_restore_entries()
        if not entries:
            return
        requests = self._build_restore_requests(entries, inplace=True)
        if not requests:
            return
        reveal_target = requests[0]["dest_path"]
        job = {
            "mode": "inplace",
            "requests": requests,
            "display_target": "по месту хранения",
            "reveal_path": str(reveal_target),
            "reveal_select": len(requests) == 1,
        }
        base_target = reveal_target.parent if reveal_target.is_file() else reveal_target
        self._run_restore(target_ov=str(base_target), files_ov=[entry["path"] for entry in entries], restore_job=job)

    def _open_selected_snapshot_in_explorer(self):
        if self._closing:
            return
        if self._restore_in_progress:
            QMessageBox.warning(self, "Проводник", "WebDAV отключён во время восстановления")
            return
        if not self._cur_snap_id:
            QMessageBox.warning(self, "Проводник", "Выберите снапшот"); return
        if sys.platform == "win32" and not self._ensure_webclient():
            return
        progress = QProgressDialog("Подождите, монтирую точку…", None, 0, 0, self)
        progress.setWindowTitle("Проводник")
        progress.setCancelButton(None)
        progress.setWindowModality(Qt.WindowModal)
        progress.setMinimumDuration(0)
        progress.setAutoClose(False)
        progress.setAutoReset(False)
        progress.show()
        QApplication.processEvents()
        try:
            self.statusBar().showMessage("Подождите, монтирую точку…", 0)
            unc_root = self._ensure_snap_browser_mount()
            target = self._snap_browser_target()
            if not target:
                raise RuntimeError("Не удалось определить путь снапшота")
            self._mount_connect_session(unc_root)
            progress.setLabelText("Открываю проводник…")
            QApplication.processEvents()
            if os.name == "nt":
                if hasattr(os, "startfile"):
                    os.startfile(target)
                else:
                    subprocess.Popen(["explorer.exe", target])
            elif hasattr(os, "startfile"):
                os.startfile(target)
            else:
                subprocess.Popen(["xdg-open", target])
            self._log(f"mount open snapshot: {target}", channel="debug")
            self.statusBar().showMessage("✅ Снапшот открыт в проводнике", 5000)
        except Exception as e:
            self._log(f"❌ mount open snapshot: {e}", "ERROR", channel="debug")
            if isinstance(e, FileNotFoundError) or getattr(e, "winerror", None) == 2:
                self._refresh_snaps(show_loading=self._is_restore_tab_active())
                QMessageBox.warning(
                    self,
                    "Проводник",
                    "Выбранный снапшот уже удалён или устарел.\nСписок снапшотов обновлён.",
                )
                return
            QMessageBox.warning(self, "Проводник", f"Не удалось открыть снапшот:\n{e}")
        finally:
            progress.close()

    def _run_restore(self,target_ov=None,files_ov=None,restore_job=None):
        if not self._cur_snap_id:
            QMessageBox.warning(self,"Восстановление","Выберите снапшот"); return False
        target=(target_ov or "").strip()
        if files_ov is not None:
            files=compact_snap_paths(files_ov)
        else:
            files=self._selected_snap_paths()
        if not files:
            QMessageBox.warning(self,"Восстановление","Выберите данные для восстановления"); return False

        job = dict(restore_job or {})
        requests = list(job.get("requests") or [])
        if not requests:
            if not target:
                QMessageBox.warning(self,"Восстановление","Не определена папка восстановления"); return False
            entries = [{"path": path, "type": "unknown"} for path in files]
            requests = self._build_restore_requests(entries, target_root=Path(target), inplace=False)
            job["requests"] = requests
            job.setdefault("display_target", str(target))

        destinations = {req["snap_path"]: req["dest_path"] for req in requests}
        if files and not self._replace_all:
            conflicts=self._restore_job_conflicts(destinations)
            if conflicts:
                dlg=ConflictDlg(next(iter(conflicts.values())),self,more=len(conflicts)-1); dlg.exec_()
                if dlg.choice==ConflictDlg.CANCEL: return False
                if dlg.choice==ConflictDlg.REPLACE_ALL: self._replace_all=True
                if dlg.choice==ConflictDlg.SKIP:
                    requests=[req for req in requests if req["snap_path"] not in conflicts]
                    if not requests:
                        self._log("Все файлы пропущены")
                        return False
                    files=[req["snap_path"] for req in requests]
                    job["requests"] = requests

        display_target = job.get("display_target") or target or "выбранный путь"
        self._log(f"=== ВОССТАНОВЛЕНИЕ {self._cur_snap_id[:8]} → {display_target} | build {APP_VERSION} ===")
        self._set_restore_buttons_enabled(False)
        self._restore_in_progress = True
        cfg = self._restic_cli_config()
        if not cfg:
            self._restore_in_progress = False
            self._set_restore_buttons_enabled(True)
            return False
        total = max(1, len(requests))
        self._show_restore_progress("Восстанавливаю…", maximum=total, value=0, clear=True)
        self._restore_q = queue.Queue()
        t = threading.Thread(
            target=self._restore_worker_thread,
            args=(cfg, requests, job, self._restore_q),
            daemon=True,
        )
        t.start()
        self._restore_timer = QTimer()
        self._restore_timer.setInterval(50)
        self._restore_timer.timeout.connect(self._poll_restore_queue)
        self._restore_timer.start()
        return True

    def _on_rest_done(self,code,_):
        self._log(f"Восстановление завершено: код {code}")

    # ─── SNAPSHOT SEARCH ────────────────────────────────────────────────────

    # ─── Удалённые: правая панель повторяет левое дерево, но только с удалённым ──

    def _deleted_busy(self, on: bool):
        self.del_progress.setVisible(on)
        self.btn_del_cancel.setVisible(on)

    def _set_del_status(self, text: str):
        self.lbl_del_status.setText(text)

    def _silent_uncheck(self, btn):
        btn.blockSignals(True)
        btn.setChecked(False)
        btn.blockSignals(False)

    def _cancel_deleted_scan(self):
        self._del_gen += 1
        proc = self._del_proc
        self._del_proc = None
        if proc:
            try: proc.kill()
            except Exception: pass

    def _clear_deleted_tree(self):
        self.snap_deleted.clear()
        self._del_nodes = {}
        self._del_found = 0
        self._del_dir_mtime = {}
        self._deleted_last = None

    def _reset_folder_deleted(self) -> bool:
        """Снимок/профиль сменился: прервать сравнение и очистить правую панель."""
        was = bool(self._del_mode) or self.snap_deleted.topLevelItemCount() > 0
        self._cancel_deleted_scan()
        self._del_mode = None
        self._deleted_busy(False)
        self._clear_deleted_tree()
        self._set_del_status("")
        return was

    def _on_del_cancel(self):
        self._cancel_deleted_scan()
        self._del_mode = None
        self._deleted_busy(False)
        self._set_del_status("Отменено")

    def _deleted_target_folder(self) -> str:
        if self._using_snap_browser():
            return self._browser_current_snap_path()
        item = self.snap_tree.currentItem()
        if not item:
            return ""
        path = normalize_snap_path(item.data(0, Qt.UserRole) or "")
        if item.data(0, Qt.UserRole + 1) != "dir":
            path = normalize_snap_path(str(PurePosixPath(path).parent))
        return path

    def _deleted_host_ok(self, silent: bool) -> bool:
        idx = self.snap_combo.currentIndex()
        host = (self._snaps[idx].get("hostname") or "") if 0 <= idx < len(self._snaps) else ""
        if host and host.lower() != socket.gethostname().lower():
            if silent:
                self._set_del_status(f"Снимок с другого ПК ({host}) — сравнение с диском невозможно")
            else:
                QMessageBox.warning(self, "Удалённые",
                    f"Снимок сделан на другом ПК ({host}).\nСравнение с локальным диском невозможно.")
            return False
        return True

    def _left_item_by_path(self, path: str) -> Optional[QTreeWidgetItem]:
        """Строка левого дерева по пути снимка (если эта ветка уже загружена)."""
        items = [self.snap_tree.topLevelItem(i) for i in range(self.snap_tree.topLevelItemCount())]
        cur = None
        for part in [p for p in path.split("/") if p]:
            cur = next((it for it in items if it.data(0, Qt.UserRole + 5) == part), None)
            if cur is None:
                return None
            items = [cur.child(i) for i in range(cur.childCount())]
        return cur

    def _right_ensure(self, path: str) -> QTreeWidgetItem:
        """Строка-«путь» в правом дереве (папка-предок): выглядит как слева, но не восстанавливается."""
        path = normalize_snap_path(path)
        it = self._del_nodes.get(path)
        if it is not None:
            return it
        pure = PurePosixPath(path)
        parent_path = normalize_snap_path(str(pure.parent))
        left = self._left_item_by_path(path)
        date = left.text(2) if left else self._format_snap_mtime(self._del_dir_mtime.get(path, ""))
        it = QTreeWidgetItem([f"📁 {pure.name}", "", date])
        it.setData(0, Qt.UserRole, path)
        it.setData(0, Qt.UserRole + 1, "dir")
        it.setData(0, Qt.UserRole + 5, pure.name)
        it.setData(0, Qt.UserRole + 6, "ctx")
        if parent_path in ("", "/"):
            self.snap_deleted.addTopLevelItem(it)
        else:
            self._right_ensure(parent_path).addChild(it)
        it.setExpanded(True)
        self._del_nodes[path] = it
        return it

    def _right_add(self, obj: dict):
        path = normalize_snap_path(obj.get("path", ""))
        if not path or path in self._del_nodes:
            return
        pure = PurePosixPath(path)
        is_dir = obj.get("type") == "dir"
        parent = self._right_ensure(normalize_snap_path(str(pure.parent)))
        size = "" if is_dir else _fmt_bytes_ui(obj.get("size") or 0)
        item = QTreeWidgetItem([f"{'📁' if is_dir else '📄'} {pure.name}", size,
                                self._format_snap_mtime(obj.get("mtime", ""))])
        item.setData(0, Qt.UserRole, path)
        item.setData(0, Qt.UserRole + 1, "dir" if is_dir else "file")
        item.setData(0, Qt.UserRole + 5, pure.name)
        item.setData(0, Qt.UserRole + 6, "deleted")
        item.setForeground(0, QColor("#b3261e"))
        item.setToolTip(0, path + "\nНет на диске")
        parent.addChild(item)
        if is_dir:
            self._del_nodes[path] = item

    def _sort_right(self, parent: QTreeWidgetItem):
        kids = parent.takeChildren()
        kids.sort(key=lambda k: (0 if k.data(0, Qt.UserRole + 1) == "dir" else 1,
                                 str(k.data(0, Qt.UserRole + 5) or "").lower()))
        parent.addChildren(kids)
        for k in kids:
            if k.childCount():
                self._sort_right(k)

    def _add_deleted_batch(self, gen: int, batch: list):
        if gen != self._del_gen:
            return
        for obj in batch:
            self._right_add(obj)
        self._del_found += len(batch)

    # --- запуск: кнопка / галка / выбор другой папки при включённой галке

    def _on_show_deleted_clicked(self, _checked=False):
        self._start_folder_scan(silent=False)

    def _on_snap_current_changed(self, cur, _prev):
        if cur is not None and self.chk_only_deleted.isChecked():
            self._auto_scan_timer.start()

    def _on_only_deleted_toggled(self, checked: bool):
        if checked:
            if not self._deleted_host_ok(False):
                self._silent_uncheck(self.chk_only_deleted)
                return
            if self._deleted_target_folder() in ("", "/"):
                self._set_del_status("Выберите папку слева — справа появятся её удалённые файлы")
                return
            self._start_folder_scan(silent=True)
        else:
            self._auto_scan_timer.stop()
            self._cancel_deleted_scan()
            self._del_mode = None
            self._deleted_busy(False)
            self._clear_deleted_tree()
            self._set_del_status("")

    def _start_folder_scan(self, silent: bool):
        folder = self._deleted_target_folder()
        if not self._cur_snap_id or not folder or folder == "/":
            if not silent:
                QMessageBox.information(self, "Показать удалённые",
                    "Выделите папку в снимке слева и нажмите кнопку.")
            return
        if not self._deleted_host_ok(silent):
            return
        if silent and (self._deleted_last == (self._cur_snap_id, folder)
                       or (self._del_mode == "folder" and self._deleted_scanned_folder == folder)):
            return   # эту папку уже показываем / считаем
        cfg = self._restic_cli_config()
        if not cfg:
            return
        self._cancel_deleted_scan()
        gen = self._del_gen
        self._del_mode = "folder"
        self._deleted_scanned_folder = folder
        self._clear_deleted_tree()
        self.snap_deleted.scrollToItem(self._right_ensure(folder))   # папка видна справа сразу
        self._set_del_status("Проверено файлов: 0")
        self._deleted_busy(True)
        threading.Thread(target=self._folder_deleted_worker, args=(cfg, self._cur_snap_id, folder, gen), daemon=True).start()

    def _folder_deleted_worker(self, cfg: dict, sid: str, folder: str, gen: int):
        # Фоновый поток: виджеты трогать нельзя, только через _ui_call_signal.
        ui = self._ui_call_signal.emit
        proc = None
        try:
            local_root = self._snap_path_to_windows_path(folder)
            if not local_root:
                ui(lambda: self._on_folder_deleted_done(gen, sid, folder, 0, {}, "Не удалось определить локальный путь папки"))
                return
            cut_len = len(folder.rstrip("/")) + 1
            # Папка и всё поддерево. Читаются только метаданные выбранной ветки.
            proc = subprocess.Popen(
                [cfg["restic"], "ls", "--json", "--recursive", sid, folder], stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL, env=cfg["env"], encoding="utf-8", errors="replace", bufsize=1,
                **hidden_console_subprocess_kwargs())
            self._del_proc = proc
            batch: List[dict] = []
            total = 0
            checked = 0                       # сколько файлов реально сверено с диском
            gone_dirs: Dict[str, str] = {}    # каталог, которого нет на диске -> верхний такой каталог
            counts: Dict[str, int] = {}       # верхний пропавший каталог -> сколько файлов внутри
            last_flush = time.monotonic()

            def flush():
                nonlocal batch
                if batch:
                    chunk, batch = batch, []
                    ui(lambda c=chunk, g=gen: self._add_deleted_batch(g, c))

            for line in proc.stdout:
                if gen != self._del_gen:
                    return  # процесс добивает finally
                try:
                    obj = json.loads(line)
                except Exception:
                    continue
                if obj.get("struct_type") != "node":
                    continue
                path = normalize_snap_path(obj.get("path", ""))
                if not path or path == folder:
                    continue
                parent = normalize_snap_path(str(PurePosixPath(path).parent))
                is_dir = obj.get("type") == "dir"
                if is_dir:
                    self._del_dir_mtime[path] = obj.get("mtime", "")
                top = gone_dirs.get(parent)
                if top is not None:
                    # Родитель уже пропал целиком: содержимое только считаем, по файлам не проверяем.
                    if is_dir:
                        gone_dirs[path] = top
                    else:
                        counts[top] = counts.get(top, 0) + 1
                    continue
                if not is_dir:
                    checked += 1
                if not os.path.lexists(os.path.join(str(local_root), *path[cut_len:].split("/"))):
                    batch.append(obj)
                    total += 1
                    if is_dir:
                        gone_dirs[path] = path
                        counts[path] = 0
                if time.monotonic() - last_flush > 0.25:
                    flush(); last_flush = time.monotonic()
                    ui(lambda n=checked, g=gen: self._del_gen == g and self._del_mode == "folder" and self._set_del_status(
                        f"Проверено файлов: {n} · найдено удалённых: {self._del_found}"))
            rc = proc.wait()
            if gen != self._del_gen:
                return
            flush()
            if rc != 0:
                # Неполный список выдавать за «удалённых нет» нельзя.
                ui(lambda: self._on_folder_deleted_done(gen, sid, folder, 0, {}, f"restic ls завершился с кодом {rc}"))
                return
            ui(lambda: self._on_folder_deleted_done(gen, sid, folder, total, dict(counts), "", checked))
        except Exception as e:
            ui(lambda e=e: self._on_folder_deleted_done(gen, sid, folder, 0, {}, str(e)))
        finally:
            if proc is not None and proc.poll() is None:
                try: proc.kill()
                except Exception: pass

    def _on_folder_deleted_done(self, gen: int, sid: str, folder: str, total: int, counts: dict, error: str, checked: int = 0):
        if gen != self._del_gen or self._del_mode != "folder":
            return
        self._del_proc = None
        self._del_mode = None
        self._deleted_busy(False)
        if error:
            self._set_del_status(f"❌ {error[:70]}")
            return
        for path, n in counts.items():
            it = self._del_nodes.get(path)
            if it is not None:
                it.setText(1, f"файлов: {n}")
        for it in self._del_nodes.values():
            if it.data(0, Qt.UserRole + 6) == "ctx" and not it.text(2):
                it.setText(2, self._format_snap_mtime(self._del_dir_mtime.get(it.data(0, Qt.UserRole), "")))
        self._sort_right(self.snap_deleted.invisibleRootItem())
        for it in self._del_nodes.values():     # takeChildren/addChildren сбрасывает раскрытие — раскрываем путь заново
            if it.data(0, Qt.UserRole + 6) == "ctx":
                it.setExpanded(True)
        self._deleted_last = (sid, folder)
        self._set_del_status(f"Удалено: {total} · проверено файлов: {checked}" if total
                             else f"✅ Удалённых файлов не обнаружено (проверено: {checked})")

    def _on_snap_search_changed(self, text: str):
        self.btn_search_clear.setVisible(bool(text))
        if self._search_debounce:
            self._search_debounce.stop()
        if not text.strip():
            self._cancel_snap_search()
            self._set_snap_view_mode(getattr(self, "_snap_view_mode_before_search", "tree"))
            self.lbl_search_status.setText("")
            return
        self._search_debounce = QTimer(); self._search_debounce.setSingleShot(True)
        self._search_debounce.timeout.connect(lambda: self._start_snap_search(text.strip()))
        self._search_debounce.start(400)

    def _cancel_snap_search(self):
        if self._search_poll_timer:
            self._search_poll_timer.stop(); self._search_poll_timer = None
        if self._search_proc:
            try: self._search_proc.kill()
            except Exception: pass
            self._search_proc = None
        self._search_q = None

    def _start_snap_search(self, query: str):
        if not self._cur_snap_id:
            return
        self._cancel_snap_search()
        if getattr(self, "_snap_view_mode", "tree") != "search":
            self._snap_view_mode_before_search = getattr(self, "_snap_view_mode", "tree")
        self._set_snap_view_mode("search")
        self.snap_search_list.clear()
        self.lbl_search_status.setText("⏳ Поиск…")
        cfg = self._restic_cli_config()
        if not cfg:
            self.lbl_search_status.setText("❌ Нет конфигурации"); return
        self._search_q = queue.Queue()
        sid = self._cur_snap_id
        t = threading.Thread(
            target=self._snap_search_worker,
            args=(cfg, sid, query, self._search_q),
            daemon=True,
        )
        t.start()
        self._search_poll_timer = QTimer()
        self._search_poll_timer.setInterval(80)
        self._search_poll_timer.timeout.connect(self._poll_search_queue)
        self._search_poll_timer.start()

    def _snap_search_worker(self, cfg: dict, snapshot_id: str, query: str, q: "queue.Queue"):
        try:
            args = [cfg["restic"], "ls", "--json", snapshot_id]
            # stderr в тот же пайп: отдельный stderr=PIPE никто не читал, буфер
            # 64 КБ заполнялся и restic блокировался — поиск висел «⏳» вечно.
            proc = subprocess.Popen(
                args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                env=cfg["env"], encoding="utf-8", errors="replace",
                bufsize=1, **hidden_console_subprocess_kwargs(),
            )
            self._search_proc = proc
            ql = query.lower()
            count = 0
            for line in proc.stdout:
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                except Exception:
                    continue
                path = obj.get("path", "")
                if not path or obj.get("struct_type") == "snapshot":
                    continue
                if ql in path.lower() or ql in obj.get("name", "").lower():
                    size = obj.get("size", 0) or 0
                    mtime = (obj.get("mtime") or "")[:16].replace("T", " ")
                    ftype = obj.get("type", "file")
                    q.put(("result", path, size, mtime, ftype))
                    count += 1
            proc.wait()
            q.put(("done", count))
        except Exception as e:
            q.put(("error", str(e)))
        finally:
            self._search_proc = None

    def _poll_search_queue(self):
        if not self._search_q:
            return
        added = 0
        for _ in range(50):
            try:
                msg = self._search_q.get_nowait()
            except Exception:
                break
            kind = msg[0]
            if kind == "result":
                _, path, size, mtime, ftype = msg
                icon = "📁" if ftype == "dir" else "📄"
                size_s = _fmt_bytes_ui(size) if size else ""
                parts = [p for p in [size_s, mtime] if p]
                suffix = f"  [{', '.join(parts)}]" if parts else ""
                item = QListWidgetItem(f"{icon} {path}{suffix}")
                item.setData(Qt.UserRole, path)
                item.setToolTip(path)
                self.snap_search_list.addItem(item)
                added += 1
            elif kind == "done":
                count = msg[1]
                self.lbl_search_status.setText(f"Найдено: {count}")
                if self._search_poll_timer: self._search_poll_timer.stop(); self._search_poll_timer = None
                return
            elif kind == "error":
                self.lbl_search_status.setText(f"❌ {msg[1][:60]}")
                if self._search_poll_timer: self._search_poll_timer.stop(); self._search_poll_timer = None
                return
        if added:
            n = self.snap_search_list.count()
            self.lbl_search_status.setText(f"Найдено: {n}…")

    def _on_drop(self,paths,target_dir):
        target=target_dir or str(self._local_path)
        if not target: QMessageBox.warning(self,"Drop","Не определена папка назначения"); return
        self._run_restore(target_ov=target,files_ov=paths)

    # ─── INIT REPO ───────────────────────────────────────────────────────────

    def _init_repo(self, on_success=None):
        err=self._validate_req()
        if err: QMessageBox.warning(self,"Обязательные поля",err); return
        self._autosave(); self._save_global()
        self._mark_repo_saved()
        if self.pid:
            _rp = (self.cfg.profile(self.pid) or {}).get("repo_path","")
            if _rp and not any(_rp.startswith(s) for s in ("sftp:","s3:","b2:","azure:","gs:","rclone:","rest:")):
                try: Path(_rp).mkdir(parents=True, exist_ok=True)
                except Exception: pass
        w=self._mk_worker()
        if not w: return
        w.out.connect(lambda m:self._log(m))
        w.err.connect(lambda m:self._log("❌ "+m,"ERROR"))
        def _on_init_done(c,_):
            if c==0:
                if on_success:
                    on_success()
                else:
                    QMessageBox.information(self,"Репозиторий","✅ Инициализирован!")
            else:
                QMessageBox.warning(self,"Репозиторий",f"❌ Ошибка инициализации (код {c})")
        w.done.connect(_on_init_done)
        w.init(); self._workers[self.pid]=w

    # ─── LOGS ────────────────────────────────────────────────────────────────

    def _append_log_view(self, view, text: str):
        if hasattr(view, "appendPlainText"):
            view.appendPlainText(text)
        else:
            view.append(text)
        view.verticalScrollBar().setValue(view.verticalScrollBar().maximum())

    def _log(self,msg,lv="INFO",channel="debug"):
        # ИНВАРИАНТ: тело выполняется только в UI-потоке (_log_queue — обычный
        # list, виджет log_error_view). Вызов из воркер-потока пере-доставляем
        # queued-сигналом — вызывать _log можно откуда угодно.
        if threading.current_thread() is not threading.main_thread():
            self._ui_call_signal.emit(lambda: self._log(msg, lv, channel))
            return
        channel = normalize_log_channel(channel)
        if self.pid:
            self._log_queue.append((self.pid, msg, lv, channel))
        self._logs_dirty = True
        if lv == "ERROR" and hasattr(self, "log_error_view"):
            ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
            line = format_log_line(ts, lv, msg, channel=channel, show_channel=True)
            self.log_error_view.appendPlainText(line)
            self.log_error_view.verticalScrollBar().setValue(
                self.log_error_view.verticalScrollBar().maximum()
            )

    def _flush_log_queue(self):
        if not self._log_queue:
            return
        items, self._log_queue = self._log_queue[:], []
        for pid, msg, lv, ch in items:
            self.cfg.log(pid, msg, lv, channel=ch)

    def _ref_logs(self):
        if self.pid:
            info_text="".join(self.cfg.get_logs(self.pid, mode="info"))
            debug_text="".join(self.cfg.get_logs(self.pid, mode="debug"))
            error_text="".join(self.cfg.get_logs(self.pid, mode="error"))
            if hasattr(self, "log_info_view"):
                self.log_info_view.setPlainText(info_text)
                self.log_info_view.verticalScrollBar().setValue(self.log_info_view.verticalScrollBar().maximum())
            if hasattr(self, "log_debug_view"):
                self.log_debug_view.setPlainText(debug_text)
                self.log_debug_view.verticalScrollBar().setValue(self.log_debug_view.verticalScrollBar().maximum())
            if hasattr(self, "log_error_view"):
                self.log_error_view.setPlainText(error_text)
                self.log_error_view.verticalScrollBar().setValue(self.log_error_view.verticalScrollBar().maximum())
            self._logs_dirty = False

    def _clr_logs(self):
        if self.pid:
            self.cfg.clr_logs(self.pid)
            if hasattr(self, "log_info_view"): self.log_info_view.clear()
            if hasattr(self, "log_debug_view"): self.log_debug_view.clear()
            if hasattr(self, "log_error_view"): self.log_error_view.clear()

    def _add_failed_files_to_exclude(self):
        if not self.pid:
            QMessageBox.information(self, "Исключения", "Профиль не выбран")
            return
        text = ""
        try:
            text = self.log_error_view.toPlainText() or ""
        except Exception:
            pass
        if not text.strip():
            try:
                text = "".join(self.cfg.get_logs(self.pid, mode="error"))
            except Exception:
                text = ""
        if not text.strip():
            QMessageBox.information(self, "Исключения", "В логе Error пусто — нечего добавлять.")
            return
        # Извлекаем все JSON-объекты с полем "item" из restic-ошибок.
        candidates = []
        for line in text.splitlines():
            s = line.find("{")
            if s < 0:
                continue
            chunk = line[s:]
            try:
                d = json.loads(chunk)
            except Exception:
                # Попробовать вычленить "item":"..." регуляркой как fallback
                m = re.search(r'"item"\s*:\s*"((?:\\.|[^"\\])*)"', chunk)
                if m:
                    raw = m.group(1).encode("utf-8", "ignore").decode("unicode_escape", errors="ignore")
                    candidates.append(raw)
                continue
            if isinstance(d, dict):
                item = d.get("item")
                if isinstance(item, str) and item.strip():
                    candidates.append(item)
        if not candidates:
            QMessageBox.information(self, "Исключения", "В логе нет проблемных файлов (поле item не найдено).")
            return
        # Нормализуем: чистим VSS-префикс \\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy*\...
        def _normalize(p: str) -> str:
            p = p.strip().replace("/", "\\")
            m = re.match(r"^\\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy\d+\\(.*)$", p, re.IGNORECASE)
            if m:
                # Восстанавливаем как X:\path — берём первый сегмент как диск-имя?
                # В реальности нам неизвестна буква диска источника, поэтому подставим
                # путь относительно корня. Если источник профиля начинается на букве — припишем её.
                tail = m.group(1)
                prof = self.cfg.profile(self.pid) or {}
                for src in prof.get("sources", []) or []:
                    s = src.replace("/", "\\")
                    if len(s) >= 2 and s[1] == ":":
                        return s[:3] + tail if not tail.startswith("\\") else s[:2] + tail
                return tail
            return p
        new_paths = []
        seen = set()
        for p in candidates:
            n = _normalize(p)
            if n and n not in seen:
                seen.add(n)
                new_paths.append(n)
        prof = self.cfg.profile(self.pid) or {}
        cur = list(prof.get("exclude", []) or [])
        cur_norm = {c.replace("/", "\\").lower() for c in cur}
        added = []
        for p in new_paths:
            if p.replace("/", "\\").lower() not in cur_norm:
                added.append(p)
                cur.append(p)
                cur_norm.add(p.replace("/", "\\").lower())
        if not added:
            QMessageBox.information(self, "Исключения",
                f"Найдено {len(new_paths)} проблемных файлов — все уже в исключениях.")
            return
        prof["exclude"] = cur
        self.cfg.save_profile(self.pid, prof)
        # Обновляем UI списка исключений
        if hasattr(self, "excl_list"):
            self.excl_list.clear()
            self.excl_list.addItems([s.replace("/", "\\") for s in cur])
        preview = "\n".join("• " + p for p in added[:15])
        more = f"\n…и ещё {len(added) - 15}" if len(added) > 15 else ""
        QMessageBox.information(self, "Исключения",
            f"Добавлено в исключения: {len(added)}\n(уже было: {len(new_paths) - len(added)})\n\n{preview}{more}")

    # ─── TELEGRAM ────────────────────────────────────────────────────────────

    def _save_tgf(self):
        self.cfg.tg_fatal=self.chk_tf.isChecked()
        self.cfg.tg_file=self.chk_te.isChecked()
        self.cfg.tg_prune=self.chk_tp.isChecked()
        self._autosave(include_repo=False)

    def _save_tg(self):
        self.cfg.tg_token=self.ed_tgt.text().strip()
        self.cfg.tg_chat=self.ed_tgc.text().strip()
        if hasattr(self, "ed_tg_group"):
            self.cfg.tg_group=self.ed_tg_group.text().strip()
        self.cfg.report_url=self.ed_report_url.text().strip()
        self.cfg.agent_auto_update_url=self.ed_agent_update_url.text().strip()
        self._save_tgf(); self.lbl_tgs.setText("💾 Сохранено")

    def _test_tg(self):
        # Сетевой вызов уносим в поток — не морозим UI Qt (до 10с таймаут).
        # Результат доставляем СИГНАЛОМ (pyqtSignal), а НЕ QTimer.singleShot из
        # рабочего потока: в потоке нет Qt event loop, поэтому singleShot там не
        # срабатывает и колбэк не вызывается → часы «⏳» висят вечно (это и был баг).
        # Сигнал, созданный в main-потоке, доставляется через очередь в main-поток.
        # seq-guard игнорирует запоздалый результат прошлой (неверной) проверки.
        self.lbl_tgs.setText("⏳…")
        token=self.ed_tgt.text().strip()
        chat=self.ed_tgc.text().strip()
        group=self.ed_tg_group.text().strip() if hasattr(self,"ed_tg_group") else ""
        lbl = self.lbl_tgs            # конкретный label этой вкладки
        seq = getattr(self, "_tg_test_seq", 0) + 1
        self._tg_test_seq = seq

        class _TgSig(QObject):
            done = pyqtSignal(bool, str)
        sig = _TgSig()
        self._tg_test_sig = sig       # держим ссылку, чтобы GC не убил до доставки

        def _result(ok, err):
            if seq != getattr(self, "_tg_test_seq", 0):
                return  # устаревший результат предыдущей проверки — игнорируем
            if ok:
                lbl.setText("✅ Установлено")
            else:
                lbl.setText("❌ Ошибка")
                QMessageBox.warning(self, "Telegram", f"Ошибка:\n{err}")
        sig.done.connect(_result)

        def _worker():
            try:
                ok, err = tg_test(token, chat, group)
            except Exception as e:
                ok, err = False, str(e)
            sig.done.emit(ok, err)

        threading.Thread(target=_worker, daemon=True).start()

    def _http_test(self, lbl, do_request):
        """Фоновый HTTP-тест соединения: do_request()->(ok,msg) в потоке, результат
        в main-поток через pyqtSignal (НЕ QTimer.singleShot из потока — он там не
        срабатывает). lbl — QLabel для статуса."""
        lbl.setText("⏳…"); lbl.setStyleSheet("color:#555;")
        class _Sig(QObject):
            done = pyqtSignal(bool, str)
        sig = _Sig()
        # Раньше сигнал хранился в ОДНОМ поле, которое переиспользовали
        # _test_report_server и _test_update_server: два быстрых клика — и первый
        # _Sig собирался GC до доставки, метка навсегда «⏳». Держим список.
        if not hasattr(self, "_http_test_sigs"):
            self._http_test_sigs = []
        self._http_test_sigs.append(sig)
        def _result(ok, msg):
            lbl.setText(("✅ " if ok else "❌ ") + msg)
            lbl.setStyleSheet("color:#006600;" if ok else "color:#cc0000;")
            try:
                self._http_test_sigs.remove(sig)
            except Exception:
                pass
        sig.done.connect(_result)
        def _worker():
            try:
                ok, msg = do_request()
            except Exception as e:
                ok, msg = False, str(e)
            sig.done.emit(bool(ok), str(msg))
        threading.Thread(target=_worker, daemon=True).start()

    def _test_report_server(self):
        """Тест сервера отчётов: POST {report_url} с {} — сервер сперва проверяет
        ключ (401=неверный), затем профиль (400 missing profile = ключ ОК, запись
        НЕ создаётся)."""
        # Кнопка «Проверить» одновременно СОХРАНЯЕТ URL+ключ в конфиг: значение
        # пишется в config агента (применяется на ближайшем цикле, без перезапуска
        # службы) и в конфиг веб-сервера отчётов.
        self._wsvr_save_config()
        url = _normalize_report_url(self.ed_report_url.text().strip())
        key = self.ed_wsvr_api_key.text().strip()
        def req():
            if not url:
                return (False, "Report URL не задан")
            headers = {"Content-Type": "application/json"}
            if key:
                headers["X-Api-Key"] = key
            code = _http_post_code(url, b"{}", headers, timeout=8)
            if code == 400:
                return (True, "сервер доступен, ключ принят")
            if code == 401:
                return (False, "неверный API-ключ (401)")
            if 200 <= code < 300:
                return (True, f"сервер доступен ({code})")
            return (False, f"HTTP {code}")
        self._http_test(self.lbl_wsvr_status, req)

    def _test_update_server(self):
        """Тест сервера обновлений: GET {url}/api/manifest с X-Api-Key."""
        # Кнопка «Проверить» одновременно СОХРАНЯЕТ URL+ключ обновлений в конфиг
        # агента — применяется на ближайшем цикле проверки без перезапуска службы.
        self._upd_save_config()
        base = self.ed_update_url.text().strip().rstrip("/")
        key = self.ed_update_key.text().strip()
        def req():
            if not base:
                return (False, "URL сервера не задан")
            headers = {}
            if key:
                headers["X-Api-Key"] = key
            code = _http_get_code(base + "/api/manifest", headers, timeout=8)
            if 200 <= code < 300:
                return (True, "сервер доступен, ключ принят")
            if code == 401:
                return (False, "неверный API-ключ (401)")
            return (False, f"HTTP {code}")
        self._http_test(self.lbl_upd_test, req)

    def _pname(self):
        if not self.pid: return ""
        p=self.cfg.profile(self.pid); return p.get("name","") if p else ""

    def _show_cli_help(self):
        QMessageBox.information(self, "Ключи командной строки",
            "<b>RestLex CLI</b><br><br>"
            "<code>restlex.exe</code> — запуск GUI<br>"
            "<code>restlex.exe --help</code> — справка<br>"
            "<code>restlex.exe --version</code> — версия<br>"
            "<code>restlex.exe --run-backup &lt;pid&gt;</code> — автозапуск бэкапа<br>"
            "<code>restlex.exe --run-backup &lt;pid&gt; --no-scan</code> — без скана<br>"
            "<code>restlex.exe --run-backup &lt;pid&gt; --check</code> — с check после<br>"
            "<code>restlex.exe --run-backup &lt;pid&gt; --pre-cmd \"cmd\"</code> — команда перед<br>"
            "<code>restlex.exe install --all</code> — установка (без ярлыка)<br>"
            "<code>restlex.exe install --all --user &lt;name&gt;</code> — установка + ярлык юзеру<br>"
            "<code>restlex.exe export-config path.json</code><br>"
            "<code>restlex.exe import-config path.json</code>")

    def _run_cli_backup(self):
        pid = getattr(self, "_cli_run_backup_pid", None)
        if not pid:
            return
        # Override global settings with CLI flags temporarily
        if getattr(self, "_cli_no_scan", False):
            self.cfg.global_no_scan = True
            if hasattr(self, "chk_global_no_scan"):
                self.chk_global_no_scan.setChecked(True)
        if getattr(self, "_cli_check", False):
            self.cfg.global_post_backup_check = True
            if hasattr(self, "chk_global_post_check"):
                self.chk_global_post_check.setChecked(True)
        pre = getattr(self, "_cli_pre_cmd", "")
        if pre:
            self.cfg.global_pre_backup_cmd = pre
            if hasattr(self, "edit_global_pre_cmd"):
                self.edit_global_pre_cmd.setPlainText(pre)
        # Select profile
        found = False
        for i in range(self.plist.count()):
            it = self.plist.item(i)
            if it and it.data(Qt.UserRole) == pid:
                self.plist.setCurrentRow(i)
                self._on_prof_click(it)
                found = True
                break
        if not found:
            self._log(f"Профиль {pid} не найден для --run-backup", "ERROR", channel="error")
            return
        QTimer.singleShot(300, self._run_backup)

    # ─── AGENT SERVICE ───────────────────────────────────────────────────────

    def _agent_set_status(self, text: str):
        self.lbl_agent_status.setText(text)
        low = text.lower()
        if "running" in low or "запущ" in low:
            self.lbl_agent_status.setStyleSheet("color:green;font-weight:bold;")
        elif "не установлена" in low or "не найден" in low:
            self.lbl_agent_status.setStyleSheet("color:gray;font-weight:bold;")
        else:
            self.lbl_agent_status.setStyleSheet("color:#885500;font-weight:bold;")

    def _agent_refresh_status(self):
        """Обновляем статус агента в фоне — не блокируем GUI."""
        import threading
        def _do():
            text = agent_status_string()
            self._ui_call_signal.emit(lambda: self._agent_set_status(text))
        threading.Thread(target=_do, daemon=True).start()

    def _agent_sync_local(self, quiet=False):
        try:
            path = sync_local_agent_binary()
            if not path or not path.exists():
                raise FileNotFoundError("release restlex-agent.exe не найден")
            if not quiet:
                self.statusBar().showMessage(f"✅ Агент обновлён: {path.name}", 4000)
            self._agent_refresh_status()
        except Exception as e:
            self._agent_set_status("Агент не готов")
            if not quiet:
                QMessageBox.warning(self, "RestlexAgent", f"Не удалось синхронизировать агент:\n{e}")

    def _agent_exec(self, args: List[str], ok_msg: str):
        """Запускает команду агента в фоне — не блокирует GUI."""
        import threading
        def _do():
            try:
                sync_local_agent_binary()
                out = run_agent_cmd(args)
                def _ok():
                    self.statusBar().showMessage(ok_msg, 5000)
                    self._log(f"agent {' '.join(args)}: {out}", channel="debug")
                    self._agent_refresh_status()
                    self._refresh_service_badge()
                self._ui_call_signal.emit(_ok)
            except Exception as e:
                err = str(e)
                def _err():
                    self._log(f"❌ agent {' '.join(args)}: {err}", "ERROR", channel="debug")
                    QMessageBox.warning(self, "RestlexAgent", err)
                    self._agent_refresh_status()
                self._ui_call_signal.emit(_err)
        threading.Thread(target=_do, daemon=True).start()

    def _agent_install(self):
        self._agent_exec(["install"], "✅ Служба установлена")

    def _agent_uninstall(self):
        self._agent_exec(["uninstall"], "🗑 Служба удалена")

    def _agent_start(self):
        self._agent_exec(["start"], "▶ Служба запущена")

    def _agent_stop(self):
        self._agent_exec(["stop"], "⏹ Служба остановлена")

    def _agent_install_and_start(self):
        """Устанавливает (если нужно) и запускает службу в фоне."""
        import threading
        def _do():
            try:
                sync_local_agent_binary()
                try:
                    status = run_agent_cmd(["status"])
                    already_installed = "не установлена" not in status.lower()
                except Exception:
                    already_installed = False
                if not already_installed:
                    run_agent_cmd(["install"])
                run_agent_cmd(["start"])
                def _ok():
                    self.statusBar().showMessage("▶ Служба запущена", 5000)
                    self._agent_refresh_status()
                    self._refresh_service_badge()
                self._ui_call_signal.emit(_ok)
            except Exception as e:
                err = str(e)
                def _err():
                    QMessageBox.warning(self, "RestlexAgent", err)
                    self._agent_refresh_status()
                self._ui_call_signal.emit(_err)
        threading.Thread(target=_do, daemon=True).start()

    def _startup_agent_check(self):
        """Проверяет статус службы при старте.
        Предлагает запустить (если установлена но остановлена) или установить+запустить (если не установлена)."""
        import threading
        def _do():
            try:
                sync_local_agent_binary()
            except Exception:
                pass
            try:
                status = run_agent_cmd(["status"])
                low = status.lower()
                if "не установлена" in low:
                    installed, stopped, running = False, False, False
                else:
                    installed = True
                    running = "running" in low
                    stopped = not running
            except Exception:
                installed = False
                stopped = False
                running = False
            if running:
                return
            def _ui():
                if installed and stopped:
                    if QMessageBox.question(
                        self, "RestlexAgent",
                        "Служба RestlexAgent установлена, но не запущена.\nЗапустить сейчас?",
                        QMessageBox.Yes | QMessageBox.No
                    ) == QMessageBox.Yes:
                        self._agent_start()
                elif not installed:
                    if QMessageBox.question(
                        self, "RestlexAgent",
                        "Служба RestlexAgent не установлена.\nУстановить и запустить сейчас?",
                        QMessageBox.Yes | QMessageBox.No
                    ) == QMessageBox.Yes:
                        self._agent_install_and_start()
            self._ui_call_signal.emit(_ui)
        threading.Thread(target=_do, daemon=True).start()

    def _refresh_service_badge(self):
        """Async: обновляет бейдж в шапке. Использует sc.exe напрямую (без restlex-agent.exe)."""
        import threading
        def _do():
            state = "missing"
            try:
                kw = {"capture_output": True, "text": True, "timeout": 5,
                      "encoding": "utf-8", "errors": "replace"}
                kw.update(hidden_console_subprocess_kwargs())
                r = subprocess.run(["sc", "query", "RestlexAgent"], **kw)
                out = (r.stdout or "") + (r.stderr or "")
                up = out.upper()
                if r.returncode != 0 or "1060" in out or "DOES NOT EXIST" in up or "НЕ СУЩЕСТВУЕТ" in up:
                    state = "missing"
                elif "RUNNING" in up:
                    state = "ok"
                elif "STOPPED" in up or "PENDING" in up or "PAUSED" in up:
                    state = "stopped"
                else:
                    state = "missing"
            except Exception:
                state = "missing"
            self._service_badge_signal.emit(state)
        threading.Thread(target=_do, daemon=True).start()

    def _apply_service_badge(self, state: str):
        # Бейдж — компактный, виден только при проблемах
        if hasattr(self, "lbl_service_badge"):
            if state == "ok":
                self.lbl_service_badge.setVisible(False)
            else:
                tip = ("Служба не установлена" if state == "missing"
                       else "Служба остановлена") + " — кликните для управления"
                self.lbl_service_badge.setText(
                    '<a href="#open" style="color:white;text-decoration:none;">Service-Off</a>'
                )
                self.lbl_service_badge.setStyleSheet(
                    "background:#c0392b;color:white;font-weight:bold;font-size:9pt;"
                    "border-radius:3px;padding:2px 8px;margin:2px 4px 2px 0;"
                )
                self.lbl_service_badge.setToolTip(tip)
                self.lbl_service_badge.setVisible(True)
        # Синхронизируем статус в Настройки → Интеграции (lbl_agent_status)
        if hasattr(self, "lbl_agent_status"):
            if state == "ok":
                self.lbl_agent_status.setText("✓ Запущена")
                self.lbl_agent_status.setStyleSheet("color:#1b6e1b;font-weight:bold;")
            elif state == "stopped":
                self.lbl_agent_status.setText("⏹ Остановлена")
                self.lbl_agent_status.setStyleSheet("color:#c0392b;font-weight:bold;")
            else:
                self.lbl_agent_status.setText("✗ Не установлена")
                self.lbl_agent_status.setStyleSheet("color:gray;font-weight:bold;")

    def _open_service_settings(self, _link=None):
        """Переключает на Настройки → Интеграции."""
        if hasattr(self, "_tab_settings"):
            self.tabs.setCurrentWidget(self._tab_settings)
        if hasattr(self, "_settings_inner_tabs"):
            it = self._settings_inner_tabs
            for i in range(it.count()):
                if "Интеграции" in it.tabText(i):
                    it.setCurrentIndex(i)
                    break

    def _ensure_agent_running_prompt(self) -> bool:
        """Проверяет что служба RestlexAgent запущена. Если нет — предлагает установить."""
        # sc/HTTP/sleep — в фоновом потоке (_run_bg_wait), диалоги остаются в UI.
        # _run_bg_wait умеет бросить RuntimeError по жёсткому таймауту — в слоте
        # Qt это необработанное исключение, поэтому оборачиваем.
        try:
            if self._run_bg_wait(agent_api_health):
                return True
            status = (self._run_bg_wait(agent_status_string) or "").lower()
        except Exception as e:
            QMessageBox.warning(self, "RestlexAgent",
                                f"Не удалось проверить состояние службы:\n{e}")
            return False
        msg = (
            "Служба RestlexAgent не запущена.\n\n"
            "Она необходима для работы серверов.\n"
            "Установить и запустить службу сейчас?"
        )
        if QMessageBox.question(self, "RestlexAgent", msg,
                                QMessageBox.Yes | QMessageBox.No) != QMessageBox.Yes:
            return False
        def _work():
            if "не установлена" in status:
                run_agent_cmd(["install"])
            run_agent_cmd(["start"])
            for _ in range(15):
                if agent_api_health():
                    return True
                time.sleep(0.4)
            return False
        try:
            if self._run_bg_wait(_work):
                self._agent_refresh_status()
                return True
            QMessageBox.warning(self, "RestlexAgent", "Служба не ответила за 6 секунд.")
            return False
        except Exception as e:
            QMessageBox.warning(self, "RestlexAgent", f"Не удалось запустить службу:\n{e}")
            return False

    # ─── MOUNT / WEBDAV ──────────────────────────────────────────────────────

    def _webclient_running(self) -> bool:
        try:
            r = subprocess.run(["sc", "query", "WebClient"],
                               capture_output=True, text=True, timeout=5,
                               **hidden_console_subprocess_kwargs())
            return "RUNNING" in (r.stdout or "")
        except Exception:
            return True

    def _webclient_feature_installed(self) -> bool:
        """True если компонент WebDAV/WebClient вообще присутствует в системе."""
        try:
            r = subprocess.run(["sc", "query", "WebClient"],
                               capture_output=True, text=True, timeout=5,
                               **hidden_console_subprocess_kwargs())
            return r.returncode == 0 and "does not exist" not in (r.stdout + r.stderr).lower()
        except Exception:
            return True

    def _webclient_pending_reboot(self) -> bool:
        """True если компонент был включён но ещё не было ребута (ключ PendingFileRenameOperations или DISM pending)."""
        try:
            import winreg
            k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE,
                               r"SYSTEM\CurrentControlSet\Control\Session Manager")
            val, _ = winreg.QueryValueEx(k, "PendingFileRenameOperations")
            winreg.CloseKey(k)
            return bool(val)
        except Exception:
            return False

    def _ensure_webclient(self) -> bool:
        """Проверяет WebClient/WebDAV.
        - Если запущена — возвращает True.
        - Если служба есть но остановлена — предлагает запустить.
        - Если компонент не установлен — предлагает включить через DISM (требует ребут).
        - Возвращает True только если служба в итоге RUNNING."""
        if sys.platform != "win32":
            return True
        # sc query / dism блокируют UI-поток (до 5с и 120с). Гоним их через
        # _run_bg_wait: работа в фоновом потоке, UI-поток крутит локальный
        # event loop и не замерзает. Диалоги остаются в UI-потоке.
        # Проверяем ожидающий ребут (компонент включён, но ещё не активен)
        # _run_bg_wait бросает RuntimeError по жёсткому таймауту — в слоте Qt это
        # необработанное исключение, поэтому все три проверки под одним try.
        try:
            feature_installed = None
            if self._webclient_pending_reboot():
                feature_installed = self._run_bg_wait(self._webclient_feature_installed)
                if not feature_installed:
                    QMessageBox.warning(
                        self, "WebDAV",
                        "Компонент WebDAV был включён, но требуется перезагрузка.\n"
                        "Пожалуйста, перезагрузите компьютер и повторите попытку."
                    )
                    return False
            if self._run_bg_wait(self._webclient_running):
                return True
            if feature_installed is None:
                feature_installed = self._run_bg_wait(self._webclient_feature_installed)
        except Exception as e:
            QMessageBox.warning(self, "WebDAV",
                                f"Не удалось проверить службу WebClient:\n{e}")
            return False
        if not feature_installed:
            # Компонент не установлен — предлагаем включить через DISM
            if QMessageBox.question(
                self, "WebDAV — компонент не установлен",
                "Компонент WebDAV Client (WebClient) не установлен в системе.\n\n"
                "Он необходим для открытия репозитория в Проводнике.\n"
                "На серверных Windows его нужно включить через DISM.\n\n"
                "Включить компонент сейчас? (потребуется перезагрузка)",
                QMessageBox.Yes | QMessageBox.No
            ) != QMessageBox.Yes:
                return False
            try:
                QApplication.setOverrideCursor(Qt.WaitCursor)
                r = self._run_bg_wait(lambda: subprocess.run(
                    ["dism", "/Online", "/Enable-Feature",
                     "/FeatureName:WebDAVRedirector", "/NoRestart"],
                    capture_output=True, text=True, timeout=120,
                    **hidden_console_subprocess_kwargs()
                ))
                QApplication.restoreOverrideCursor()
                if r.returncode not in (0, 3010):
                    QMessageBox.warning(
                        self, "WebDAV",
                        f"DISM завершился с кодом {r.returncode}.\n{r.stdout[-800:]}"
                    )
                    return False
                QMessageBox.information(
                    self, "WebDAV",
                    "Компонент WebDAV включён.\n\n"
                    "⚠ Требуется перезагрузка компьютера для завершения установки.\n"
                    "После перезагрузки повторите попытку."
                )
            except Exception as e:
                QApplication.restoreOverrideCursor()
                QMessageBox.warning(self, "WebDAV", f"Не удалось включить компонент:\n{e}")
            return False
        # Компонент установлен, но служба остановлена — предлагаем запустить
        if QMessageBox.question(
            self, "WebDAV",
            "Служба WebClient установлена, но не запущена.\n"
            "Запустить службу сейчас?",
            QMessageBox.Yes | QMessageBox.No
        ) != QMessageBox.Yes:
            return False
        def _start_webclient():
            subprocess.run(["sc", "start", "WebClient"],
                           capture_output=True, timeout=10,
                           **hidden_console_subprocess_kwargs())
            import time as _t
            for _ in range(8):
                _t.sleep(0.5)
                if self._webclient_running():
                    return True
            return False
        try:
            if self._run_bg_wait(_start_webclient):
                return True
            QMessageBox.warning(self, "WebDAV", "Служба WebClient не запустилась.")
            return False
        except Exception as e:
            QMessageBox.warning(self, "WebDAV", f"Не удалось запустить WebClient:\n{e}")
            return False

    def _mount_apply_status(self, info=None, err_text=""):
        if not hasattr(self, "lbl_mount"):
            return
        info = info or {}
        active = bool(info.get("active"))
        pid = info.get("pid")
        name = info.get("profile_name") or ""
        unc = self._mount_unc_from_info(info)
        self._mount_user = (info.get("username") or "").strip()
        self._mount_password = (info.get("password") or "").strip()
        if err_text:
            self.lbl_mount.setText(f"⚠ {err_text}")
            self.lbl_mount.setStyleSheet("color:#885500;font-weight:bold;")
            self._snap_browser_unc_root = ""
            self._snap_browser_unc_target = ""
            self._snap_browser_last_target = ""
            self._close_snap_browser()
            self.ed_mount_path.clear()
            self.btn_mount_open.setEnabled(False)
            self.btn_mount_stop.setEnabled(False)
            self.btn_mount_start.setEnabled(bool(self.pid))
            return
        if active:
            self._snap_browser_unc_root = unc.rstrip("\\/")
            if pid == self.pid:
                self.lbl_mount.setText(f"✅ Подключён текущий профиль: {name}")
                self.lbl_mount.setStyleSheet("color:green;font-weight:bold;")
            else:
                self.lbl_mount.setText(f"⚠ Подключён другой профиль: {name}")
                self.lbl_mount.setStyleSheet("color:#885500;font-weight:bold;")
            self.ed_mount_path.setText(unc)
            self.btn_mount_open.setEnabled(bool(unc))
            self.btn_mount_stop.setEnabled(True)
            self.btn_mount_start.setEnabled(bool(self.pid))
            return
        self.lbl_mount.setText("● mount не подключён")
        self.lbl_mount.setStyleSheet("color:gray;font-weight:bold;")
        self._snap_browser_unc_root = ""
        self._snap_browser_unc_target = ""
        self._snap_browser_last_target = ""
        self._close_snap_browser()
        self.ed_mount_path.clear()
        self.btn_mount_open.setEnabled(False)
        self.btn_mount_stop.setEnabled(False)
        self.btn_mount_start.setEnabled(bool(self.pid))

    def _mount_unc_from_info(self, info=None) -> str:
        info = info or {}
        unc = str(info.get("unc_path") or "").strip()
        port = info.get("port")
        if unc and "@" in unc:
            return unc
        # Агент биндит WebDAV на localhost — UNC вида \\PCNAME@port\ пошёл бы на
        # LAN-адрес и не достучался бы. Только localhost.
        host = "localhost"
        if port:
            fallback = rf"\\{host}@{port}\DavWWWRoot"
            if not unc:
                return fallback
            m = re.match(r"^\\\\([^\\]+)\\DavWWWRoot$", unc, re.I)
            if m:
                server = m.group(1)
                if "@" not in server:
                    port_text = str(port)
                    if server.endswith(port_text):
                        server = server[:-len(port_text)] or host
                    else:
                        server = host
                    return rf"\\{server}@{port_text}\DavWWWRoot"
        return unc

    def _mount_cmd(self, args: List[str], timeout=20, input_text=None):
        kw = {
            "capture_output": True,
            "text": True,
            "encoding": "utf-8",
            "errors": "replace",
            "timeout": timeout,
        }
        if input_text is not None:
            kw["input"] = input_text
        kw.update(hidden_console_subprocess_kwargs())
        return subprocess.run(args, **kw)

    def _mount_disconnect_session(self, unc: str):
        unc = (unc or "").strip()
        if not unc or os.name != "nt":
            return
        try:
            self._mount_cmd(["net", "use", unc, "/delete", "/y"], timeout=10)
        except Exception:
            pass

    def _stop_mount_quiet(self, fast=False):
        unc = ""
        if hasattr(self, "ed_mount_path"):
            unc = self.ed_mount_path.text().strip()
        status_timeout = 0.35 if fast else 3
        stop_timeout = 0.6 if fast else 10
        disconnect_timeout = 0.6 if fast else 10
        try:
            info = agent_api_request("/mount/status", timeout=status_timeout)
            if isinstance(info, dict) and info.get("active"):
                unc = (self._mount_unc_from_info(info) or unc).strip()
                try:
                    agent_api_request("/mount/stop", method="POST", timeout=stop_timeout)
                except Exception:
                    pass
        except Exception:
            pass
        if unc:
            try:
                if fast:
                    subprocess.Popen(
                        ["net", "use", unc, "/delete", "/y"],
                        **hidden_console_subprocess_kwargs(),
                    )
                else:
                    self._mount_cmd(["net", "use", unc, "/delete", "/y"], timeout=disconnect_timeout)
            except Exception:
                pass
        self._mount_apply_status({})

    def _mount_connect_session(self, unc: str):
        unc = (unc or "").strip()
        if not unc or os.name != "nt":
            return False
        # Basic-авторизация на WebDAV агента СНЯТА (см. restlex-agent): Windows
        # WebClient по умолчанию имеет BasicAuthLevel=1 — креды отдаёт только
        # поверх SSL, и `net use ... /user:` по plain HTTP падал с 1244 у всех,
        # то есть монтирование было сломано полностью. Защита остаётся за счёт
        # bind на 127.0.0.1. Пустые username/password в MountStatus — норма,
        # блокировать монтирование из-за них НЕЛЬЗЯ.
        user = (getattr(self, "_mount_user", "") or "").strip()
        password = (getattr(self, "_mount_password", "") or "").strip()
        self._mount_disconnect_session(unc)
        if _MOUNT_BASIC_AUTH and user and password:
            # Путь со stdin-паролем сохранён на случай возврата Basic (пароль НЕ
            # в argv: `net use ... <pw>` виден любому через wmic commandline).
            r = self._mount_cmd(
                ["net", "use", unc, "*", f"/user:{user}", "/persistent:no"],
                timeout=20,
                input_text=password + "\r\n",
            )
        else:
            r = self._mount_cmd(["net", "use", unc, "/persistent:no"], timeout=20)
        txt = ((r.stdout or "") + "\n" + (r.stderr or "")).strip()
        if r.returncode != 0:
            raise RuntimeError(txt or f"net use вернул код {r.returncode}")
        return True

    def _ensure_agent_api(self):
        # Всё блокирующее (sc/subprocess до 30с + HTTP + sleep 0.4×12) уносим в
        # поток; UI-поток крутит локальный event loop и остаётся живым.
        self._agent_sync_local(quiet=True)
        if agent_api_health():
            return
        def _work():
            status = agent_status_string().lower()
            if "не установлена" in status:
                run_agent_cmd(["install"])
            run_agent_cmd(["start"])
            for _ in range(12):
                if agent_api_health():
                    return True
                time.sleep(0.4)
            return False
        if self._run_bg_wait(_work):
            self._agent_refresh_status()
            return
        raise RuntimeError(f"API RestlexAgent не отвечает на {AGENT_API_BASE}")

    def _mount_refresh_status(self):
        if not hasattr(self, "lbl_mount"):
            return
        if self._restore_in_progress:
            return
        try:
            info = agent_api_request("/mount/status", timeout=3)
            if not isinstance(info, dict):
                info = {}
            self._mount_apply_status(info)
        except Exception:
            self._mount_apply_status({}, "служба mount недоступна")

    def _mount_open(self):
        if self._closing:
            return
        if self._restore_in_progress:
            QMessageBox.warning(self, "Mount", "WebDAV отключён во время восстановления")
            return
        unc = self.ed_mount_path.text().strip()
        if not unc:
            self._mount_refresh_status()
            unc = self.ed_mount_path.text().strip()
        if not unc:
            QMessageBox.warning(self, "Mount", "Путь mount ещё не получен"); return
        try:
            connected = self._mount_connect_session(unc)
            if os.name == "nt":
                if hasattr(os, "startfile"):
                    os.startfile(unc)
                else:
                    subprocess.Popen(["explorer.exe", unc])
            elif hasattr(os, "startfile"):
                os.startfile(unc)
            else:
                subprocess.Popen(["xdg-open", unc])
            suffix = " через net use" if connected else ""
            self._log(f"mount open: {unc}{suffix}", channel="debug")
        except Exception as e:
            self._log(f"❌ mount open: {e}", "ERROR", channel="debug")
            QMessageBox.warning(self, "Mount", f"Не удалось открыть Explorer:\n{e}")

    def _mount_start(self):
        if self._closing:
            return
        if self._restore_in_progress:
            QMessageBox.warning(self, "Mount", "WebDAV отключён во время восстановления")
            return
        if not self.pid:
            QMessageBox.warning(self, "Mount", "Выберите профиль"); return
        if sys.platform == "win32" and not self._ensure_webclient():
            return
        try:
            self.statusBar().showMessage("⏳ Поднимаю WebDAV mount…", 0)
            self._ensure_agent_api()
            data = agent_api_request(
                f"/mount/start/{urllib.parse.quote(self.pid)}",
                method="POST",
                timeout=20,
            )
            if not isinstance(data, dict) or not data.get("ok"):
                raise RuntimeError((data or {}).get("error", "mount не запустился"))
            info = data.get("mount") or {}
            self._mount_apply_status(info)
            self._log(f"mount start: {info.get('unc_path','')}", channel="debug")
            self.statusBar().showMessage("✅ Mount подключён", 5000)
            if info.get("unc_path"):
                self._mount_open()
        except Exception as e:
            self._mount_apply_status({}, str(e))
            self._log(f"❌ mount start: {e}", "ERROR", channel="debug")
            QMessageBox.warning(self, "Mount", f"Не удалось подключить mount:\n{e}")

    def _mount_stop(self):
        try:
            unc = self.ed_mount_path.text().strip()
            self._ensure_agent_api()
            data = agent_api_request("/mount/stop", method="POST", timeout=10)
            if not isinstance(data, dict) or not data.get("ok"):
                raise RuntimeError((data or {}).get("error", "mount не остановился"))
            self._mount_disconnect_session(unc)
            self._mount_apply_status(data.get("mount") or {})
            self._log("mount stop", channel="debug")
            self.statusBar().showMessage("⏹ Mount отключён", 4000)
        except Exception as e:
            self._log(f"❌ mount stop: {e}", "ERROR", channel="debug")
            QMessageBox.warning(self, "Mount", f"Не удалось отключить mount:\n{e}")
            self._mount_refresh_status()

    # ─── SCHEDULER TAB ───────────────────────────────────────────────────────

    def _build_scheduler(self):
        tab = QWidget(); ly = QVBoxLayout(tab); ly.setSpacing(8)

        self.lbl_sched_status = QLabel("Выберите профиль")
        self.lbl_sched_status.setStyleSheet("color:gray;font-style:italic;")
        self.lbl_sched_status.setWordWrap(True)
        self.lbl_sched_status.setVisible(False)

        self.lbl_agent_offline = QLabel(
            "⚠ Служба RestlexAgent не запущена — планировщик не работает.  "
            "<a href='#start'>Установить и запустить</a>"
        )
        self.lbl_agent_offline.setOpenExternalLinks(False)
        self.lbl_agent_offline.linkActivated.connect(lambda _: self._agent_install_and_start())
        self.lbl_agent_offline.setStyleSheet(
            "color:#7a3800;font-weight:bold;background:#fff3cd;"
            "padding:5px 8px;border:1px solid #ffc107;border-radius:3px;"
        )
        self.lbl_agent_offline.setWordWrap(True)
        self.lbl_agent_offline.setVisible(False)
        ly.addWidget(self.lbl_agent_offline)

        rg = QGroupBox("Расписание"); rgl = QVBoxLayout(rg)
        fr = QHBoxLayout()
        self._sched_bg = QButtonGroup(self)
        for i, (lbl, val) in enumerate([("Ежедневно","daily"),("Еженедельно","weekly"),
                                         ("Каждые N часов","hourly"),("При входе","onlogon")]):
            rb = QRadioButton(lbl); rb.setProperty("freq", val)
            self._sched_bg.addButton(rb, i); fr.addWidget(rb)
        fr.addStretch()
        self._sched_bg.buttons()[0].setChecked(True)
        self._sched_bg.buttonClicked.connect(self._sched_toggle_ui)
        rgl.addLayout(fr)

        self._sched_time_row = QWidget()
        trl = QVBoxLayout(self._sched_time_row); trl.setContentsMargins(0,0,0,0); trl.setSpacing(3)
        input_row = QHBoxLayout()
        input_row.addWidget(QLabel("Время запуска:"))
        self.sched_time = QTimeEdit(); self.sched_time.setDisplayFormat("HH:mm")
        self.sched_time.setTime(QTime(2, 0)); self.sched_time.setFixedWidth(80)
        input_row.addWidget(self.sched_time)
        btn_add_t = QPushButton("+"); btn_add_t.setFixedWidth(28); btn_add_t.setToolTip("Добавить время в расписание")
        btn_add_t.clicked.connect(self._sched_add_time)
        input_row.addWidget(btn_add_t)
        self.btn_sched_del_time = QPushButton("−"); self.btn_sched_del_time.setFixedWidth(28)
        self.btn_sched_del_time.setToolTip("Удалить выбранное время")
        self.btn_sched_del_time.clicked.connect(self._sched_del_time)
        input_row.addWidget(self.btn_sched_del_time)
        input_row.addStretch()
        trl.addLayout(input_row)
        self.sched_times_list = QListWidget(); self.sched_times_list.setFixedHeight(66)
        self.sched_times_list.setToolTip("Список запланированных времён. Для каждого создаётся отдельная задача.")
        self.sched_times_list.addItem("02:00")
        trl.addWidget(self.sched_times_list)
        rgl.addWidget(self._sched_time_row)

        self._sched_days_row = QWidget()
        drl = QHBoxLayout(self._sched_days_row); drl.setContentsMargins(0,0,0,0)
        drl.addWidget(QLabel("Дни:"))
        self._day_chks = {}
        for code, name in [("MON","Пн"),("TUE","Вт"),("WED","Ср"),("THU","Чт"),
                            ("FRI","Пт"),("SAT","Сб"),("SUN","Вс")]:
            cb = QCheckBox(name); cb.setProperty("day", code)
            self._day_chks[code] = cb; drl.addWidget(cb)
        self._day_chks["MON"].setChecked(True)
        drl.addStretch()
        rgl.addWidget(self._sched_days_row)
        self._sched_days_row.setVisible(False)

        self._sched_interval_row = QWidget()
        irl = QHBoxLayout(self._sched_interval_row); irl.setContentsMargins(0,0,0,0)
        irl.addWidget(QLabel("Интервал:"))
        self.sched_interval = QSpinBox(); self.sched_interval.setRange(1, 24)
        self.sched_interval.setValue(4); self.sched_interval.setSuffix(" ч")
        self.sched_interval.setFixedWidth(80)
        irl.addWidget(self.sched_interval); irl.addStretch()
        rgl.addWidget(self._sched_interval_row)
        self._sched_interval_row.setVisible(False)
        ly.addWidget(rg)

        ug = QGroupBox("Запуск от имени"); ugl = QVBoxLayout(ug)
        self._sched_user_bg = QButtonGroup(self)
        self.rb_sched_user_cur = QRadioButton("Текущего пользователя")
        self.rb_sched_user_cur.setProperty("run_as", "current")
        self.rb_sched_user_cur.setToolTip("Задача запускается от имени текущего пользователя Windows.")
        self.rb_sched_user_sys = QRadioButton("SYSTEM (служба)")
        self.rb_sched_user_sys.setProperty("run_as", "system")
        self.rb_sched_user_sys.setToolTip("Задача запускается от имени SYSTEM — без пароля, работает в фоне даже без входа пользователя.")
        self._sched_user_bg.addButton(self.rb_sched_user_cur, 0)
        self._sched_user_bg.addButton(self.rb_sched_user_sys, 1)
        self.rb_sched_user_cur.setChecked(True)
        ugl.addWidget(self.rb_sched_user_cur)
        # ponytail: пароль учётки убран — задачи исполняет служба-агент, schtasks не создаются,
        # так что вводить и хранить пароль Windows было незачем (он всё равно не сохранялся).
        ugl.addWidget(self.rb_sched_user_sys)
        ly.addWidget(ug)

        og = QGroupBox("Дополнительно"); ogl = QVBoxLayout(og)
        self.chk_sched_tg = QCheckBox("Уведомлять в Telegram при ошибке бэкапа")
        self.chk_sched_tg.setChecked(True)
        self.lbl_sched_tg_warn = QLabel(
            "⚠ Не заполнены данные Telegram → Настройки › Интеграции"
        )
        self.lbl_sched_tg_warn.setStyleSheet("color:#885500;font-size:9pt;padding-left:20px;")
        self.lbl_sched_tg_warn.setVisible(False)
        self.chk_sched_forget = QCheckBox("Выполнять очистку репозитория согласно правилам хранения")
        self.chk_sched_forget.setChecked(True)
        ogl.addWidget(self.chk_sched_tg)
        ogl.addWidget(self.lbl_sched_tg_warn)
        ogl.addWidget(self.chk_sched_forget)
        self.chk_sched_forget.toggled.connect(lambda c: self._sched_render_preview_cmd())
        ly.addWidget(og)

        def _update_tg_warn(checked):
            tg_ok = bool(self.cfg.tg_token and self.cfg.tg_chat)
            self.lbl_sched_tg_warn.setVisible(checked and not tg_ok)
        self.chk_sched_tg.toggled.connect(_update_tg_warn)
        _update_tg_warn(self.chk_sched_tg.isChecked())

        # ── Параметры запуска (overlay над профилем/глобалом) ───────────────
        pg = QGroupBox("⚙ Изменить параметры запуска")
        pg.setCheckable(True); pg.setChecked(False)
        pg_inner = QWidget()
        pgl = QVBoxLayout(pg_inner); pgl.setContentsMargins(0, 0, 0, 0)
        pg_v = QVBoxLayout(pg); pg_v.addWidget(pg_inner)
        pg.toggled.connect(pg_inner.setVisible)
        pg_inner.setVisible(False)
        # Запоминаем ручной выбор пользователя — не закрываем группу автоматически
        pg.toggled.connect(lambda c: setattr(self, "_sched_params_user_opened", bool(c)))
        self._sched_params_user_opened = False
        self._sched_params_group = pg

        row1 = QHBoxLayout()
        self.chk_p_no_scan = QCheckBox("--no-scan (без сканирования)")
        self.chk_p_use_vss = QCheckBox("--use-fs-snapshot (VSS)")
        self.chk_p_pre_unlock = QCheckBox("Проверка блокировок")
        row1.addWidget(self.chk_p_no_scan); row1.addWidget(self.chk_p_use_vss); row1.addWidget(self.chk_p_pre_unlock); row1.addStretch()
        pgl.addLayout(row1)

        row2 = QHBoxLayout()
        self.chk_p_force_orphan = QCheckBox("Снимать брошенные локи")
        self.chk_p_pre_repair = QCheckBox("repair index перед бэкапом")
        row2.addWidget(self.chk_p_force_orphan); row2.addWidget(self.chk_p_pre_repair); row2.addStretch()
        pgl.addLayout(row2)

        row3 = QHBoxLayout()
        row3.addWidget(QLabel("Лок брошен после:"))
        self.spin_p_stale_grace = QSpinBox(); self.spin_p_stale_grace.setRange(1, 1440); self.spin_p_stale_grace.setSuffix(" мин"); self.spin_p_stale_grace.setFixedWidth(110)
        row3.addWidget(self.spin_p_stale_grace); row3.addStretch()
        pgl.addLayout(row3)

        # ponytail: два простыни-поля с длинными подписями раздували группу —
        # прячем в свою сворачиваемую группу, пояснения уехали в tooltip
        cg = QGroupBox("Команды restic (для правки вручную)")
        cg.setCheckable(True); cg.setChecked(False)
        cg_inner = QWidget()
        cgl = QVBoxLayout(cg_inner); cgl.setContentsMargins(0, 0, 0, 0)
        QVBoxLayout(cg).addWidget(cg_inner)
        cg.toggled.connect(cg_inner.setVisible)
        cg_inner.setVisible(False)

        mono = "font-family:Consolas,monospace;font-size:9.5pt;background:#fafafa;"
        lbl_bk = QLabel("Бэкап:")
        lbl_bk.setToolTip("Правь как хочешь — exclude и доп. ключи берутся отсюда; при сохранении парсится.")
        cgl.addWidget(lbl_bk)
        self.ed_p_preview = QPlainTextEdit()
        self.ed_p_preview.setMinimumHeight(90); self.ed_p_preview.setMaximumHeight(140)
        self.ed_p_preview.setStyleSheet(mono)
        self.ed_p_preview.setToolTip(lbl_bk.toolTip())
        cgl.addWidget(self.ed_p_preview)

        lbl_fg = QLabel("После бэкапа (очистка):")
        lbl_fg.setToolTip("Правь правила хранения — переопределит профиль для этой задачи.")
        cgl.addWidget(lbl_fg)
        self.ed_p_preview_forget = QPlainTextEdit()
        self.ed_p_preview_forget.setMinimumHeight(70); self.ed_p_preview_forget.setMaximumHeight(110)
        self.ed_p_preview_forget.setStyleSheet(mono)
        self.ed_p_preview_forget.setToolTip(lbl_fg.toolTip())
        cgl.addWidget(self.ed_p_preview_forget)
        pgl.addWidget(cg)

        prow = QHBoxLayout()
        self.btn_p_reset = QPushButton("✖ Сбросить на наследование от профиля")
        self.btn_p_reset.clicked.connect(self._sched_reset_overlay)
        prow.addWidget(self.btn_p_reset); prow.addStretch()
        pgl.addLayout(prow)

        for w in (self.chk_p_no_scan, self.chk_p_use_vss, self.chk_p_pre_unlock,
                  self.chk_p_force_orphan, self.chk_p_pre_repair):
            w.toggled.connect(lambda _c: self._sched_render_preview_cmd())
        self.spin_p_stale_grace.valueChanged.connect(lambda _v: self._sched_render_preview_cmd())

        ly.addWidget(pg)

        br = QHBoxLayout()
        self.btn_sched_create = QPushButton("✅ Создать задачу с текущими параметрами")
        self.btn_sched_create.setFixedWidth(290)
        self.btn_sched_create.clicked.connect(lambda: self._sched_apply(update_existing=False))
        self.btn_sched_update = QPushButton("💾 Сохранить параметры задачи")
        self.btn_sched_update.setFixedWidth(260)
        self.btn_sched_update.setEnabled(False)
        self.btn_sched_update.clicked.connect(lambda: self._sched_apply(update_existing=True))
        br.addWidget(self.btn_sched_create)
        br.addWidget(self.btn_sched_update)
        br.addStretch()
        ly.addLayout(br)

        mg = QGroupBox("Менеджер задач"); mgl = QVBoxLayout(mg)
        self.sched_tasks = QTreeWidget()
        self.sched_tasks.setRootIsDecorated(False)
        self.sched_tasks.setAlternatingRowColors(True)
        self.sched_tasks.setSelectionMode(QAbstractItemView.SingleSelection)
        self.sched_tasks.setHeaderLabels(["Задача", "Метод", "Расписание", "Prune", "Telegram", "От имени", "Опции", "Следующий запуск", "Статус"])
        self.sched_tasks.header().setSectionResizeMode(0, QHeaderView.ResizeToContents)
        self.sched_tasks.header().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.sched_tasks.header().setSectionResizeMode(2, QHeaderView.Stretch)
        self.sched_tasks.header().setSectionResizeMode(3, QHeaderView.ResizeToContents)
        self.sched_tasks.header().setSectionResizeMode(4, QHeaderView.ResizeToContents)
        self.sched_tasks.header().setSectionResizeMode(5, QHeaderView.ResizeToContents)
        self.sched_tasks.header().setSectionResizeMode(6, QHeaderView.ResizeToContents)
        self.sched_tasks.header().setSectionResizeMode(7, QHeaderView.ResizeToContents)
        self.sched_tasks.header().setSectionResizeMode(8, QHeaderView.ResizeToContents)
        self.sched_tasks.itemSelectionChanged.connect(self._sched_on_selection_changed)
        mgl.addWidget(self.sched_tasks)
        mr = QHBoxLayout()
        self.btn_sched_run = QPushButton("▶ Запустить")
        self.btn_sched_run.setFixedWidth(130)
        self.btn_sched_run.setEnabled(False)
        self.btn_sched_run.clicked.connect(self._sched_run_now)
        self.btn_sched_del = QPushButton("🗑 Удалить")
        self.btn_sched_del.setFixedWidth(130)
        self.btn_sched_del.setEnabled(False)
        self.btn_sched_del.clicked.connect(self._sched_delete)
        self.btn_sched_ref = QPushButton("🔄 Обновить")
        self.btn_sched_ref.setFixedWidth(130)
        self.btn_sched_ref.clicked.connect(self._sched_refresh_status)
        mr.addWidget(self.btn_sched_run)
        mr.addWidget(self.btn_sched_del)
        mr.addWidget(self.btn_sched_ref)
        mr.addStretch()
        mgl.addLayout(mr)
        ly.addWidget(mg)
        ly.addStretch()
        # ponytail: вкладка без скролла разъезжалась при раскрытии "Параметры запуска"
        # в невысоком окне — обёртка та же, что у вкладок настроек
        sc = QScrollArea(); sc.setWidgetResizable(True); sc.setFrameShape(QFrame.NoFrame)
        sc.setWidget(tab)
        self._tab_scheduler = sc
        self.tabs.addTab(sc, "⏰ Планировщик")

        # Авто-опрос статуса планировщика каждые 3 секунды
        self._sched_auto_timer = QTimer(self)
        self._sched_auto_timer.setInterval(3000)
        self._sched_auto_timer.timeout.connect(self._sched_auto_refresh)

    def _build_settings(self):
        tab = QWidget(); ly = QVBoxLayout(tab); ly.setSpacing(4)
        self._tab_settings = tab
        inner_tabs = QTabWidget()
        self._settings_inner_tabs = inner_tabs

        # ── Вкладка 1: Параметры бэкапа ─────────────────────────────────────
        t1 = QScrollArea(); t1.setWidgetResizable(True)
        t1w = QWidget(); t1l = QVBoxLayout(t1w); t1l.setSpacing(6)
        fl = QFormLayout(); fl.setSpacing(6); fl.setLabelAlignment(Qt.AlignRight)

        lbl_r = QLabel("Путь к restic"); lbl_r.setToolTip("Путь к restic.exe. Пустое поле — используется встроенный restic из папки программы.")
        self.edit_restic = QLineEdit(self.cfg.gg("restic", ""))
        self.edit_restic.setPlaceholderText(f"Пусто = {self.cfg.managed_restic_path()}")
        self.edit_restic.textEdited.connect(self._changed)
        self.edit_restic.textChanged.connect(lambda _: self._refresh_restic_status_label())
        self.edit_restic.editingFinished.connect(self._save_global)
        btn_rb = QPushButton("📂"); btn_rb.setFixedWidth(30)
        btn_rb.clicked.connect(lambda: (self.edit_restic.setText(
            QFileDialog.getOpenFileName(self, "restic.exe", filter="*.exe")[0] or self.edit_restic.text()), self._changed()))
        row_r = QHBoxLayout(); row_r.addWidget(self.edit_restic, 1); row_r.addWidget(btn_rb)
        fl.addRow(lbl_r, row_r)
        self.btn_restic_check = QPushButton("🔎 Проверить версию")
        self.btn_restic_check.clicked.connect(self._check_restic_version)
        self.btn_restic_download = QPushButton("⬇ Загрузить")
        self.btn_restic_download.clicked.connect(self._download_restic_release)
        row_rd = QHBoxLayout(); row_rd.addWidget(self.btn_restic_check); row_rd.addWidget(self.btn_restic_download); row_rd.addStretch()
        fl.addRow("", row_rd)
        self.lbl_restic_status = QLabel(""); self.lbl_restic_status.setWordWrap(True); self.lbl_restic_status.setStyleSheet("color:#555;")
        fl.addRow("", self.lbl_restic_status)

        lbl_t = QLabel("TEMP"); lbl_t.setToolTip("Папка для временных файлов restic. Пустое поле — системный TEMP.")
        self.edit_temp = QLineEdit(self.cfg.temp); self.edit_temp.setPlaceholderText("Пусто = системный TEMP")
        self.edit_temp.textEdited.connect(self._changed); self.edit_temp.editingFinished.connect(self._save_global)
        btn_tb = QPushButton("📂"); btn_tb.setFixedWidth(30)
        btn_tb.clicked.connect(lambda: (self.edit_temp.setText(
            QFileDialog.getExistingDirectory(self, "TEMP") or self.edit_temp.text()), self._changed()))
        row_t = QHBoxLayout(); row_t.addWidget(self.edit_temp, 1); row_t.addWidget(btn_tb)
        fl.addRow(lbl_t, row_t)

        sep1 = QFrame(); sep1.setFrameShape(QFrame.HLine); sep1.setStyleSheet("color:#ccc;")
        fl.addRow(sep1)

        def _chk_row(name, chk_widget, desc):
            w = QWidget(); l = QHBoxLayout(w); l.setContentsMargins(0,0,0,0); l.setSpacing(6)
            l.addWidget(chk_widget)
            d = QLabel(desc); d.setStyleSheet("color:#666;font-size:8pt;"); d.setWordWrap(True)
            l.addWidget(d, 1)
            return w

        self.chk_vss = QCheckBox(); self.chk_vss.setChecked(True); self.chk_vss.toggled.connect(self._changed)
        fl.addRow("VSS", _chk_row("VSS", self.chk_vss, "Volume Shadow Copy — теневой снимок диска. Позволяет копировать файлы, открытые другими программами."))

        self.sp_vss_timeout = QSpinBox(); self.sp_vss_timeout.setRange(30, 3600); self.sp_vss_timeout.setSuffix(" сек")
        self.sp_vss_timeout.setValue(self.cfg.global_vss_timeout_s); self.sp_vss_timeout.setFixedWidth(110)
        self.sp_vss_timeout.valueChanged.connect(self._changed); self.sp_vss_timeout.valueChanged.connect(self._save_global)
        _lbl_vt = QLabel("Сколько ждать создания теневой копии. Дефолт restic — 120 сек, на нагруженном сервере мало.")
        _lbl_vt.setWordWrap(True); _lbl_vt.setStyleSheet("color:#666;font-size:11px;")
        _rvt = QHBoxLayout(); _rvt.addWidget(self.sp_vss_timeout); _rvt.addWidget(_lbl_vt, 1)
        fl.addRow("Таймаут VSS", _rvt)

        self.chk_global_no_scan = QCheckBox(); self.chk_global_no_scan.setChecked(self.cfg.global_no_scan); self.chk_global_no_scan.toggled.connect(self._changed); self.chk_global_no_scan.toggled.connect(self._save_global)
        fl.addRow("--no-scan", _chk_row("--no-scan", self.chk_global_no_scan, "Пропустить сканирование источника перед бэкапом. Ускоряет старт, прогресс менее точный."))

        self.chk_global_post_check = QCheckBox(); self.chk_global_post_check.setChecked(self.cfg.global_post_backup_check); self.chk_global_post_check.toggled.connect(self._changed); self.chk_global_post_check.toggled.connect(self._save_global)
        fl.addRow("--check", _chk_row("--check", self.chk_global_post_check, "После успешного бэкапа запускать «restic check» — проверка целостности репозитория."))

        self.chk_check_host_only = QCheckBox(); self.chk_check_host_only.setChecked(self.cfg.global_check_host_only); self.chk_check_host_only.toggled.connect(self._changed); self.chk_check_host_only.toggled.connect(self._save_global)
        # Модификатор, а не альтернатива: без «--check» проверять нечего.
        self.chk_check_host_only.setEnabled(self.cfg.global_post_backup_check)
        self.chk_global_post_check.toggled.connect(self.chk_check_host_only.setEnabled)
        fl.addRow("--check только свой хост", _chk_row("--check только свой хост", self.chk_check_host_only, "Проверять только снимки этой машины (restic check --host). Ускоряет проверку общего репозитория, куда пишут несколько машин. Требуется restic 0.19.0 или новее."))

        self.chk_pre_repair = QCheckBox(); self.chk_pre_repair.setChecked(True); self.chk_pre_repair.toggled.connect(self._changed)
        fl.addRow("repair index", _chk_row("repair index", self.chk_pre_repair, "Перед бэкапом выполнять «restic repair index» — восстановление индекса репозитория."))

        self.chk_pre_unlock = QCheckBox(); self.chk_pre_unlock.setChecked(True); self.chk_pre_unlock.toggled.connect(self._changed)
        fl.addRow("Проверка блокировок", _chk_row("Проверка блокировок", self.chk_pre_unlock, "Перед бэкапом проверять блокировки репозитория. Если идёт другой бэкап — отменить, если лок брошенный — снять."))

        self.chk_force_unlock_orphan = QCheckBox(); self.chk_force_unlock_orphan.setChecked(True); self.chk_force_unlock_orphan.toggled.connect(self._changed)
        fl.addRow("Снимать брошенные локи", _chk_row("Снимать брошенные локи", self.chk_force_unlock_orphan, "Авто-снятие блокировки, если процесс restic уже не работает или лок старше указанного ниже времени."))

        self.spin_stale_grace = QSpinBox(); self.spin_stale_grace.setRange(1, 1440); self.spin_stale_grace.setValue(35); self.spin_stale_grace.setSuffix(" мин"); self.spin_stale_grace.valueChanged.connect(self._changed)
        fl.addRow("Лок брошен после", _chk_row("Лок брошен после", self.spin_stale_grace, "На чужом ПК: если лок старше этого времени — снять. По умолчанию 35 минут."))

        self.chk_cache_cleanup = QCheckBox(); self.chk_cache_cleanup.setChecked(self.cfg.cache_cleanup); self.chk_cache_cleanup.toggled.connect(self._changed); self.chk_cache_cleanup.toggled.connect(self._save_global)
        fl.addRow("cache --cleanup", _chk_row("cache --cleanup", self.chk_cache_cleanup, "После успешного бэкапа запускать «restic cache --cleanup» — очищает устаревший кэш репозитория."))

        self.cmb_language = QComboBox()
        self.cmb_language.addItem("Русский", "ru"); self.cmb_language.addItem("English", "en")
        self.cmb_language.setCurrentIndex(1 if self.cfg.gg("language", "ru") == "en" else 0)
        self.cmb_language.setFixedWidth(160)
        self.cmb_language.activated.connect(self._set_language)
        fl.addRow("Язык / Language", self.cmb_language)

        sep2 = QFrame(); sep2.setFrameShape(QFrame.HLine); sep2.setStyleSheet("color:#ccc;")
        fl.addRow(sep2)

        self.edit_global_pre_cmd = QTextEdit()
        self.edit_global_pre_cmd.setPlainText(self.cfg.global_pre_backup_cmd)
        self.edit_global_pre_cmd.setPlaceholderText("Одна команда на строку")
        self.edit_global_pre_cmd.setMaximumHeight(90)
        self.edit_global_pre_cmd.textChanged.connect(self._changed)
        _cmd_visible = bool(self.cfg.global_pre_backup_cmd.strip())
        self.edit_global_pre_cmd.setVisible(_cmd_visible)
        btn_cmd = QPushButton("▼ CMD — команды перед бэкапом")
        btn_cmd.setCheckable(True); btn_cmd.setChecked(_cmd_visible)
        btn_cmd.toggled.connect(self.edit_global_pre_cmd.setVisible)
        self.chk_global_pre_cmd = btn_cmd
        lbl_cmd = QLabel("CMD"); lbl_cmd.setToolTip("Команды shell, выполняемые перед стартом бэкапа (по одной на строку). При ошибке любой из них — бэкап отменяется.")
        cmd_wrap = QWidget(); cmd_wl = QVBoxLayout(cmd_wrap); cmd_wl.setContentsMargins(0,0,0,0); cmd_wl.setSpacing(2)
        cmd_wl.addWidget(btn_cmd); cmd_wl.addWidget(self.edit_global_pre_cmd)
        fl.addRow(lbl_cmd, cmd_wrap)

        self.chk_tg_timeout = QCheckBox()
        self.spin_global_bk_timeout = QSpinBox()
        self.spin_global_bk_timeout.setRange(1, 24); self.spin_global_bk_timeout.setSuffix(" ч"); self.spin_global_bk_timeout.setFixedWidth(80)
        _to = self.cfg.global_backup_timeout_hours
        self.chk_tg_timeout.setChecked(_to > 0); self.spin_global_bk_timeout.setValue(_to if _to > 0 else 4)
        self.spin_global_bk_timeout.setEnabled(_to > 0)
        self.chk_tg_timeout.toggled.connect(self.spin_global_bk_timeout.setEnabled)
        self.chk_tg_timeout.toggled.connect(self._changed); self.chk_tg_timeout.toggled.connect(self._save_global)
        self.spin_global_bk_timeout.valueChanged.connect(self._changed); self.spin_global_bk_timeout.valueChanged.connect(self._save_global)
        lbl_to = QLabel("Таймаут Telegram"); lbl_to.setToolTip("Если бэкап идёт дольше указанного времени — отправить уведомление в Telegram.")
        to_wrap = QWidget(); to_wl = QHBoxLayout(to_wrap); to_wl.setContentsMargins(0,0,0,0)
        to_wl.addWidget(self.chk_tg_timeout); to_wl.addWidget(self.spin_global_bk_timeout); to_wl.addStretch()
        fl.addRow(lbl_to, to_wrap)

        # Глобальный тумблер шифрования секретов (на ВЕСЬ конфиг, не per-repo).
        # Тот же паттерн _chk_row, что и у соседних галок — выровнено в одну линию.
        self.chk_encrypt = QCheckBox(); self.chk_encrypt.toggled.connect(self._on_encrypt_toggle)
        fl.addRow("🔒 Шифрование секретов", _chk_row("Шифрование секретов", self.chk_encrypt,
            "Шифровать пароли репозиториев, Telegram-токен и API-ключи на диске (DPAPI). "
            "Глобально для всех профилей. При включении значения скрываются в интерфейсе; "
            "выключение расшифрует их в открытый текст на диске."))

        # Настройка компьютера, не профиля: агент читает sleep_on_lan_listen из globals.
        self.chk_sleep_listen = QCheckBox()
        self.chk_sleep_listen.setChecked(bool(self.cfg.gg("sleep_on_lan_listen", False)))
        self.chk_sleep_listen.toggled.connect(lambda on: (self.cfg.sg("sleep_on_lan_listen", bool(on)),
            self.statusBar().showMessage("✅ Sleep-on-LAN " + ("включён — агент начнёт слушать в течение 30 сек" if on else "выключен"), 5000)))
        fl.addRow("🔌 Выключение по сети", _chk_row("Выключение по сети", self.chk_sleep_listen,
            "Разрешить выключать ЭТОТ ПК по сети (Sleep-on-LAN): агент ловит пакет с перевёрнутым MAC "
            "(UDP 9) и выключает ПК через 30 сек. Выключить сможет любой в локальной сети."))

        t1l.addLayout(fl); t1l.addStretch(); t1.setWidget(t1w)
        self._sync_encrypt_ui()
        inner_tabs.addTab(t1, "Параметры")

        # ── Вкладка 2: Интеграции ────────────────────────────────────────────
        t2 = QScrollArea(); t2.setWidgetResizable(True)
        t2w = QWidget(); t2l = QVBoxLayout(t2w); t2l.setSpacing(8)

        tg = QGroupBox("Telegram"); tgl = QVBoxLayout(tg)
        r1 = QHBoxLayout(); lbl_tk = QLabel("API Token:"); lbl_tk.setToolTip("Токен бота Telegram. Получить у @BotFather.")
        self.ed_tgt = QLineEdit(self.cfg.tg_token); self.ed_tgt.setEchoMode(QLineEdit.Password); self.ed_tgt.textEdited.connect(self._changed)
        r1.addWidget(lbl_tk); r1.addWidget(self.ed_tgt, 1); tgl.addLayout(r1)
        r2 = QHBoxLayout(); lbl_tc = QLabel("ID чата:"); lbl_tc.setToolTip("ID чата или группы Telegram (например -100XXXXXXXXXX).")
        self.ed_tgc = QLineEdit(self.cfg.tg_chat); self.ed_tgc.setPlaceholderText("-100XXXXXXXXXX"); self.ed_tgc.textEdited.connect(self._changed)
        r2.addWidget(lbl_tc); r2.addWidget(self.ed_tgc, 1); tgl.addLayout(r2)
        r2g = QHBoxLayout(); lbl_tg2 = QLabel("Тема (topic ID):"); lbl_tg2.setToolTip("ID треда в супергруппе (опционально).")
        self.ed_tg_group = QLineEdit(self.cfg.tg_group); self.ed_tg_group.setPlaceholderText("793 (опционально)"); self.ed_tg_group.textEdited.connect(self._changed)
        r2g.addWidget(lbl_tg2); r2g.addWidget(self.ed_tg_group, 1); tgl.addLayout(r2g)
        self.ed_tgt.editingFinished.connect(self._save_tg)
        self.ed_tgc.editingFinished.connect(self._save_tg)
        self.ed_tg_group.editingFinished.connect(self._save_tg)
        rb = QHBoxLayout()
        bt2 = QPushButton("📡 Проверить"); bt2.clicked.connect(self._test_tg)
        self.lbl_tgs = QLabel("")
        rb.addWidget(bt2); rb.addWidget(self.lbl_tgs); rb.addStretch()
        tgl.addLayout(rb); t2l.addWidget(tg)

        ng = QGroupBox("Уведомления Telegram при ошибках"); ngl = QVBoxLayout(ng)
        self.chk_tf = QCheckBox("Фатальная ошибка бэкапа (errorlevel 1)"); self.chk_tf.toggled.connect(self._save_tgf)
        self.chk_te = QCheckBox("Ошибка копирования файлов (errorlevel 3)"); self.chk_te.toggled.connect(self._save_tgf)
        self.chk_tp = QCheckBox("Ошибка очистки forget (errorlevel 1 или 3)"); self.chk_tp.toggled.connect(self._save_tgf)
        for c in [self.chk_tf, self.chk_te, self.chk_tp]: ngl.addWidget(c)
        t2l.addWidget(ng)

        ag2 = QGroupBox("Служба RestlexAgent"); agl2 = QVBoxLayout(ag2)
        arow = QHBoxLayout()
        self.lbl_agent_status = QLabel("—"); self.lbl_agent_status.setStyleSheet("color:gray;font-weight:bold;")
        arow.addWidget(QLabel("Статус:")); arow.addWidget(self.lbl_agent_status); arow.addStretch(); agl2.addLayout(arow)
        abtn = QHBoxLayout()
        for lbl_s, fn, w in [("➕ Установить и запустить", self._agent_install_and_start, 190),
                               ("⏹ Остановить", self._agent_stop, 120),
                               ("🗑 Удалить", self._agent_uninstall, 120)]:
            b = QPushButton(lbl_s); b.setFixedWidth(w); b.clicked.connect(fn); abtn.addWidget(b)
        abtn.addStretch(); agl2.addLayout(abtn)
        adesc = QLabel("Фоновая служба Windows для расписаний, веб-отчётов и управления rest-server.")
        adesc.setWordWrap(True); adesc.setStyleSheet("color:#555;"); agl2.addWidget(adesc)
        t2l.addWidget(ag2)

        mg2 = QGroupBox("Mount — подключение репозитория"); mgl2 = QVBoxLayout(mg2)
        lbl_md = QLabel(
            "Открывает репозиторий restic как сетевую папку в Проводнике Windows (только чтение).\n"
            "Работает через встроенный WebDAV-сервер службы RestlexAgent — сторонние драйверы не нужны.\n"
            "После подключения папка доступна по пути вида \\\\ИМЯ-ПК@порт\\DavWWWRoot."
        )
        lbl_md.setWordWrap(True); lbl_md.setStyleSheet("color:#555;"); mgl2.addWidget(lbl_md)
        mr2 = QHBoxLayout()
        self.btn_mount_start = QPushButton("📂 Подключить"); self.btn_mount_start.clicked.connect(self._mount_start)
        self.btn_mount_open = QPushButton("🗁 Открыть"); self.btn_mount_open.clicked.connect(self._mount_open); self.btn_mount_open.setEnabled(False)
        self.btn_mount_stop = QPushButton("⏏ Отключить"); self.btn_mount_stop.clicked.connect(self._mount_stop); self.btn_mount_stop.setEnabled(False)
        self.btn_mount_ref = QPushButton("↻"); self.btn_mount_ref.setFixedWidth(28); self.btn_mount_ref.setToolTip("Обновить статус mount")
        self.btn_mount_ref.clicked.connect(self._mount_refresh_status)
        mr2.addWidget(self.btn_mount_start); mr2.addWidget(self.btn_mount_open); mr2.addWidget(self.btn_mount_stop); mr2.addWidget(self.btn_mount_ref); mr2.addStretch()
        mgl2.addLayout(mr2)
        self.lbl_mount = QLabel("● mount не подключён"); self.lbl_mount.setStyleSheet("color:gray;font-weight:bold;"); mgl2.addWidget(self.lbl_mount)
        self.ed_mount_path = QLineEdit(); self.ed_mount_path.setReadOnly(True); self.ed_mount_path.setPlaceholderText(r"\\ИМЯ-ПК@порт\DavWWWRoot"); mgl2.addWidget(self.ed_mount_path)
        t2l.addWidget(mg2)

        t2l.addStretch(); t2.setWidget(t2w)
        inner_tabs.addTab(t2, "Интеграции")

        # ── Вкладка 3: Ключи установки ───────────────────────────────────────
        t3 = QScrollArea(); t3.setWidgetResizable(True)
        t3w = QWidget(); t3l = QVBoxLayout(t3w); t3l.setSpacing(10)

        exe_path = str(Path(sys.executable).resolve() if getattr(sys, "frozen", False) else Path(__file__).resolve())
        exe_lbl = QLineEdit(exe_path); exe_lbl.setReadOnly(True)
        exe_lbl.setStyleSheet("color:#555;background:#f8fafc;border:1px solid #ddd;padding:3px 6px;border-radius:3px;font-size:11px;")
        exe_grp = QGroupBox("Путь к программе"); exe_gly = QVBoxLayout(exe_grp)
        exe_gly.addWidget(exe_lbl)
        t3l.addWidget(exe_grp)

        keys_grp = QGroupBox("Ключи командной строки"); keys_gly = QVBoxLayout(keys_grp)
        KEYS = [
            ("--install-shortcut [name]",
             "Создать ярлык RestLex на рабочем столе (если нет).\nПараметр name — отображаемое имя профиля (опционально).",
             'restlex.exe --install-shortcut "admin"'),
            ("--run-backup <profile_id>",
             "Запустить бэкап указанного профиля без GUI.",
             'restlex.exe --run-backup "000-001-MY"'),
            ("--pre-cmd \"cmd\"",
             "Выполнить команду перед бэкапом (использовать с --run-backup).",
             'restlex.exe --run-backup "pid" --pre-cmd "net use Z: \\\\nas\\backup"'),
            ("install --all",
             "Полная установка: копирование в ProgramData, установка службы RestlexAgent.\nЯрлык НЕ создаётся. Для ярлыка добавь --user <имя>.",
             "restlex.exe install --all"),
            ("install --user <name>",
             "Создать ярлык RestLex.lnk на рабочем столе указанного пользователя\n(C:\\Users\\<name>\\Desktop). Без --user ярлык не создаётся.",
             'restlex.exe install --user "lex"'),
            ("install --install-agent",
             "Установить и запустить службу RestlexAgent.",
             "restlex.exe install --install-agent"),
            ("export-config <path.json>",
             "Экспортировать весь конфиг (профили + глобальные настройки) в JSON-файл.\n"
             "С --no-secrets пароли и токены не выгружаются — безопасно класть на общую шару.",
             'restlex.exe export-config "C:\\backup\\config.json"\n'
             'restlex.exe export-config "\\\\share\\etalon.json" --no-secrets'),
            ("import-config <path.json>",
             "Импортировать конфиг из JSON-файла и попросить службу перечитать его.",
             'restlex.exe import-config "C:\\backup\\config.json"'),
            ("profiles",
             "Список профилей: id и имя. Нужен, чтобы узнать id для set --profile.",
             "restlex.exe profiles"),
            ("set <key> <value>",
             "Изменить один параметр без открытия GUI. Без --profile — глобальный параметр.\n"
             "Типовой сценарий: задать пароль репозитория после импорта эталона без секретов.",
             'restlex.exe set repo_password "SecretPw" --profile a1b2c3d4\n'
             'restlex.exe set tg_token "123:ABC"'),
            ("reload",
             "Попросить службу RestlexAgent перечитать конфиг с диска.",
             "restlex.exe reload"),
            ("--uninstall-agent",
             "Остановить и удалить службу RestlexAgent.",
             "restlex.exe --uninstall-agent"),
        ]
        for key, desc, example in KEYS:
            kg = QGroupBox(); kgl = QVBoxLayout(kg); kgl.setSpacing(2); kgl.setContentsMargins(8,6,8,6)
            lbl_k = QLabel(f"<b><code>{key}</code></b>"); lbl_k.setTextFormat(Qt.RichText)
            lbl_d = QLabel(desc); lbl_d.setWordWrap(True); lbl_d.setStyleSheet("color:#555;font-size:11px;")
            ex_row = QHBoxLayout()
            ex_edit = QLineEdit(example); ex_edit.setReadOnly(True)
            ex_edit.setStyleSheet("font-family:Consolas,monospace;font-size:11px;color:#1e3a5f;background:#f0f4ff;border:1px solid #c7d2fe;border-radius:3px;padding:2px 5px;")
            btn_copy = QPushButton("📋"); btn_copy.setFixedWidth(30); btn_copy.setToolTip("Скопировать")
            btn_copy.clicked.connect(lambda checked, t=example: QApplication.clipboard().setText(t))
            ex_row.addWidget(ex_edit, 1); ex_row.addWidget(btn_copy)
            kgl.addWidget(lbl_k); kgl.addWidget(lbl_d); kgl.addLayout(ex_row)
            keys_gly.addWidget(kg)

        sh_row = QHBoxLayout()
        btn_sh = QPushButton("🖥 Создать ярлык RestLex сейчас")
        btn_sh.setFixedWidth(240)
        btn_sh.clicked.connect(self._create_shortcut_now)
        sh_row.addWidget(btn_sh); sh_row.addStretch()
        keys_gly.addLayout(sh_row)

        t3l.addWidget(keys_grp)
        t3l.addStretch(); t3.setWidget(t3w)
        inner_tabs.addTab(t3, "Ключи установки")

        # ── Вкладка 4: О программе ───────────────────────────────────────────
        t4 = QScrollArea(); t4.setWidgetResizable(True)
        t4w = QWidget(); t4l = QVBoxLayout(t4w); t4l.setSpacing(12)

        about_html = f"""<div style="font-family:Segoe UI,sans-serif;padding:8px;">
<h2 style="color:#1e3a5f;margin-bottom:4px;">🛡 RestLex</h2>
<p style="color:#555;font-size:12px;margin-bottom:12px;">Версия {APP_VERSION} &nbsp;·&nbsp; Создатель: <b>Lex</b></p>
<p style="margin-bottom:12px;">Профессиональный GUI-клиент для <b>restic</b> — современной утилиты резервного копирования с дедупликацией и шифрованием.</p>
<h3 style="color:#1e3a5f;margin-bottom:6px;">Возможности</h3>
<ul style="margin-left:16px;line-height:1.8;color:#333;">
<li><b>Профили бэкапов</b> — неограниченное количество, каждый со своими источниками, репозиторием, исключениями и политикой хранения</li>
<li><b>Планировщик</b> — автоматические бэкапы через Планировщик задач Windows, несколько расписаний на профиль</li>
<li><b>Telegram-уведомления</b> — мгновенные алерты при ошибках, завершении бэкапа и prune-очистке</li>
<li><b>REST Server</b> — управление Restic REST Server со встроенным watchdog-перезапуском</li>
<li><b>Mount / WebDAV</b> — монтирование репозитория как сетевой папки для просмотра и поиска файлов</li>
<li><b>Восстановление</b> — навигация по снепшотам, восстановление отдельных файлов и папок</li>
<li><b>VSS</b> — поддержка теневых копий Windows для бэкапа открытых и заблокированных файлов</li>
<li><b>RestlexAgent</b> — фоновая служба Windows: расписания, watchdog процессов, HTTP API для управления</li>
<li><b>RestlexWebServer</b> — отдельный сервер отчётов о бэкапах с веб-интерфейсом, историей, группами машин и управлением пользователями</li>
<li><b>Сервер обновлений</b> — собственный RestlexUpdateServer: раздача обновлений агентов и GUI внутри сети</li>
<li><b>Инсталлер</b> — RestlexInstaller автоматически разворачивает агента, скачивает конфиг и профили с сервера обновлений</li>
<li><b>Экспорт / Импорт</b> — перенос всех настроек, профилей и учётных данных между машинами одной командой</li>
<li><b>CLI</b> — полный набор ключей для автоматизации, тихой установки и интеграции в корпоративные скрипты</li>
</ul>
<h3 style="color:#1e3a5f;margin-top:14px;margin-bottom:6px;">Компоненты</h3>
<table style="border-collapse:collapse;font-size:12px;color:#333;width:100%;">
<tr style="background:#f0f4f8;"><td style="padding:4px 8px;font-weight:bold;">restlex.exe</td><td style="padding:4px 8px;">GUI-клиент (этот файл)</td></tr>
<tr><td style="padding:4px 8px;font-weight:bold;">restlex-agent.exe</td><td style="padding:4px 8px;">Служба Windows RestlexAgent — планировщик и watchdog</td></tr>
<tr style="background:#f0f4f8;"><td style="padding:4px 8px;font-weight:bold;">restlex-webserver.exe</td><td style="padding:4px 8px;">Служба RestlexWebServer — веб-интерфейс отчётов</td></tr>
<tr><td style="padding:4px 8px;font-weight:bold;">restlex-update-server.exe</td><td style="padding:4px 8px;">Служба RestlexUpdateServer — сервер обновлений</td></tr>
<tr style="background:#f0f4f8;"><td style="padding:4px 8px;font-weight:bold;">restlex-installer.exe</td><td style="padding:4px 8px;">Установщик с тихим режимом и загрузкой конфига</td></tr>
</table>
<p style="margin-top:16px;color:#888;font-size:11px;">Основан на <a href="#" style="color:#2563eb;">restic</a> — открытом бэкап-решении</p>
</div>"""
        about_browser = QLabel(); about_browser.setText(about_html)
        about_browser.setTextFormat(Qt.RichText); about_browser.setWordWrap(True)
        about_browser.setAlignment(Qt.AlignTop)
        t4l.addWidget(about_browser); t4l.addStretch(); t4.setWidget(t4w)
        inner_tabs.addTab(t4, "О программе")

        ly.addWidget(inner_tabs, 1)
        self.tabs.addTab(tab, "⚙ Настройки")
        self._refresh_restic_status_label()
        self._agent_sync_local(quiet=True)
        self._agent_refresh_status()

    def _sched_add_time(self):
        t = self.sched_time.time().toString("HH:mm")
        for i in range(self.sched_times_list.count()):
            if self.sched_times_list.item(i).text() == t:
                return
        items = sorted({self.sched_times_list.item(i).text()
                        for i in range(self.sched_times_list.count())} | {t})
        self.sched_times_list.clear()
        for item in items:
            self.sched_times_list.addItem(item)

    def _sched_del_time(self):
        if self.sched_times_list.count() <= 1:
            return
        row = self.sched_times_list.currentRow()
        if row >= 0:
            self.sched_times_list.takeItem(row)

    # ─── REST SERVER TAB ─────────────────────────────────────────────────────

    def _build_rest_server_tab(self, tab=None):
        if tab is None:
            tab = QWidget()
        ly = QVBoxLayout(tab); ly.setSpacing(8)

        ctrl_row = QHBoxLayout()
        self._rsvr_running = False
        self._rsvr_busy = False   # операция в фоне — повторные клики игнорируем
        self.btn_rsvr_toggle = QPushButton("Активировать сервер")
        self.btn_rsvr_toggle.setFixedHeight(28)
        self.btn_rsvr_toggle.setToolTip(
            "Запускает rest-server — HTTP-сервер для restic-репозиториев.\n"
            "Сервер управляется службой restlex-agent и перезапускается автоматически."
        )
        self.btn_rsvr_toggle.clicked.connect(self._rsvr_btn_clicked)
        self.lbl_rsvr_status = QLabel("")
        ctrl_row.addWidget(self.btn_rsvr_toggle)
        ctrl_row.addWidget(self.lbl_rsvr_status)
        ctrl_row.addStretch()
        ly.addLayout(ctrl_row)

        cg = QGroupBox("Конфигурация"); cgl = QFormLayout(cg)
        self.ed_rsvr_dir = QLineEdit(); self.ed_rsvr_dir.setPlaceholderText("Папка репозиториев…")
        btn_rsvr_dir = QPushButton("…"); btn_rsvr_dir.setFixedWidth(30)
        btn_rsvr_dir.clicked.connect(lambda: self.ed_rsvr_dir.setText(
            QFileDialog.getExistingDirectory(self, "Папка репозиториев") or self.ed_rsvr_dir.text()))
        row_dir = QHBoxLayout(); row_dir.addWidget(self.ed_rsvr_dir); row_dir.addWidget(btn_rsvr_dir)
        self.ed_rsvr_port = QLineEdit("8000"); self.ed_rsvr_port.setFixedWidth(80)
        cgl.addRow("Папка репозиториев:", row_dir)
        cgl.addRow("Порт:", self.ed_rsvr_port)
        ly.addWidget(cg)

        ag = QGroupBox("Аутентификация (.htpasswd)"); agl = QFormLayout(ag)
        self.ed_rsvr_user = QLineEdit("admin"); self.ed_rsvr_user.setFixedWidth(160)
        self.ed_rsvr_pass = QLineEdit(); self.ed_rsvr_pass.setEchoMode(QLineEdit.Password)
        self.ed_rsvr_pass.setFixedWidth(160); self.ed_rsvr_pass.setPlaceholderText("Пароль…")
        agl.addRow("Логин:", self.ed_rsvr_user)
        agl.addRow("Пароль:", self.ed_rsvr_pass)
        self.lbl_rsvr_htpasswd = QLabel("Файл: не создан")
        self.lbl_rsvr_htpasswd.setStyleSheet("color:gray;font-style:italic;font-size:8pt;")
        agl.addRow("", self.lbl_rsvr_htpasswd)
        ly.addWidget(ag)

        dl_row = QHBoxLayout()
        btn_dl = QPushButton("⬇ Скачать rest-server")
        btn_dl.clicked.connect(self._rsvr_download)
        dl_row.addWidget(btn_dl); dl_row.addStretch()
        ly.addLayout(dl_row)
        ly.addStretch()
        self._rsvr_load_config()
        QTimer.singleShot(500, self._rsvr_refresh_status)

    def _rsvr_load_config(self):
        cfg = RestServerManager.load_config()
        self.ed_rsvr_dir.setText(cfg.get("data_dir", ""))
        self.ed_rsvr_port.setText(str(cfg.get("port", "8000")))
        self.ed_rsvr_user.setText(cfg.get("username", "admin"))
        self.ed_rsvr_pass.setText(cfg.get("password", ""))
        htpasswd = RestServerManager.htpasswd_path(cfg.get("data_dir", "") or ".")
        if htpasswd.exists():
            self.lbl_rsvr_htpasswd.setText(f"Файл: {htpasswd}")
            self.lbl_rsvr_htpasswd.setStyleSheet("color:#006600;font-size:8pt;")
        self._rsvr_running = bool(cfg.get("enabled", False))
        self._rsvr_update_ui()

    def _rsvr_save_config(self):
        cfg = RestServerManager.load_config()
        cfg.update({
            "data_dir": self.ed_rsvr_dir.text().strip(),
            "port": self.ed_rsvr_port.text().strip() or "8000",
            "username": self.ed_rsvr_user.text().strip() or "admin",
            "password": self.ed_rsvr_pass.text(),
        })
        RestServerManager.save_config(cfg)

    def _rsvr_update_ui(self):
        port = self.ed_rsvr_port.text().strip() or "8000"
        if self._rsvr_running:
            self.lbl_rsvr_status.setText(f"● Работает (порт {port})")
            self.lbl_rsvr_status.setStyleSheet("color:#006600;font-weight:bold;")
            self.btn_rsvr_toggle.setText("Деактивировать сервер")
        else:
            self.lbl_rsvr_status.setText("")
            self.btn_rsvr_toggle.setText("Активировать сервер")

    def _rsvr_refresh_status(self, override_on_unreachable: bool = False):
        """Опрос статуса rest-server в фоне — не блокирует GUI."""
        import threading
        def _do():
            reachable = True
            try:
                running = RestServerManager.is_running()
            except Exception:
                running, reachable = False, False
            def _ui():
                # Обновляем UI и в состоянии «остановлен»; при недоступном агенте —
                # только с override, чтобы транзиентная ошибка не сбрасывала статус.
                if reachable or override_on_unreachable:
                    self._rsvr_running = running
                    self._rsvr_update_ui()
            self._ui_call_signal.emit(_ui)
        threading.Thread(target=_do, daemon=True).start()

    def _rsvr_btn_clicked(self):
        if self._rsvr_running:
            self._rsvr_stop()
        else:
            self._rsvr_start()

    def _rsvr_start(self):
        if self._rsvr_busy:
            return
        if not self._ensure_agent_running_prompt():
            return
        data_dir = self.ed_rsvr_dir.text().strip()
        port = self.ed_rsvr_port.text().strip() or "8000"
        username = self.ed_rsvr_user.text().strip() or "admin"
        password = self.ed_rsvr_pass.text()
        if not data_dir:
            QMessageBox.warning(self, "Сервер", "Укажите папку репозиториев.")
            return
        if not password:
            QMessageBox.warning(self, "Сервер", "Введите пароль для генерации .htpasswd.")
            return
        self._rsvr_save_config()
        # HTTP к агенту (timeout 5с) — только в фоне, иначе висит весь UI
        self._rsvr_busy = True
        def _do():
            try:
                htpasswd = RestServerManager.generate_htpasswd(data_dir, username, password)
                ok, msg = RestServerManager.start(data_dir, port, htpasswd)
            except Exception as e:
                htpasswd, ok, msg = "", False, str(e)
            def _ui():
                self._rsvr_busy = False
                if htpasswd:
                    self.lbl_rsvr_htpasswd.setText(f"Файл: {htpasswd}")
                    self.lbl_rsvr_htpasswd.setStyleSheet("color:#006600;font-size:8pt;")
                self._rsvr_running = bool(ok)
                if ok:
                    self.statusBar().showMessage("▶ rest-server запущен", 5000)
                else:
                    QMessageBox.critical(self, "Сервер", f"Не удалось запустить:\n{msg}")
                self._rsvr_update_ui()
            self._ui_call_signal.emit(_ui)
        threading.Thread(target=_do, daemon=True).start()

    def _rsvr_stop(self):
        if self._rsvr_busy:
            return
        self._rsvr_busy = True
        def _do():
            try:
                RestServerManager.stop()
            except Exception:
                pass
            def _ui():
                self._rsvr_busy = False
                self._rsvr_running = False
                self._rsvr_update_ui()
                self.statusBar().showMessage("■ rest-server остановлен", 5000)
            self._ui_call_signal.emit(_ui)
        threading.Thread(target=_do, daemon=True).start()

    def _rsvr_download(self):
        # скачивание с GitHub (timeout до 120с) — в фоне, курсор-«часы» event loop не крутит
        if self._rsvr_busy:
            return
        self._rsvr_busy = True
        QApplication.setOverrideCursor(Qt.WaitCursor)
        def _do():
            try:
                target, err = RestServerManager.download_exe(), None
            except Exception as e:
                target, err = None, str(e)
            def _ui():
                self._rsvr_busy = False
                QApplication.restoreOverrideCursor()
                if err is None:
                    QMessageBox.information(self, "Server", f"rest-server скачан:\n{target}")
                else:
                    QMessageBox.critical(self, "Server", f"Ошибка скачивания:\n{err}")
            self._ui_call_signal.emit(_ui)
        threading.Thread(target=_do, daemon=True).start()

    # ── Веб-сервер ──────────────────────────────────────────────────────────
    def _build_webserver_tab(self, tab=None):
        if tab is None:
            tab = QWidget()
        ly = QVBoxLayout(tab); ly.setSpacing(8)
        self._wsvr_running = False

        cg = QGroupBox("Настройки приёма отчётов"); cgl = QFormLayout(cg)
        self.ed_report_url = QLineEdit(self.cfg.report_url)
        self.ed_report_url.setPlaceholderText("http://127.0.0.1:8797/report  (порт отчётов)")
        self.ed_report_url.textEdited.connect(self._changed)
        cgl.addRow("Report URL:", self.ed_report_url)

        api_row = QHBoxLayout()
        self.ed_wsvr_api_key = QLineEdit(); self.ed_wsvr_api_key.setFixedWidth(280)
        self.ed_wsvr_api_key.setPlaceholderText("Оставить пусто — без ключа")
        self.ed_wsvr_api_key.setEchoMode(QLineEdit.Password)
        btn_eye_wsvr = QToolButton(); btn_eye_wsvr.setText("👁"); btn_eye_wsvr.setCheckable(True)
        btn_eye_wsvr.toggled.connect(lambda v: self.ed_wsvr_api_key.setEchoMode(
            QLineEdit.Normal if v else QLineEdit.Password))
        btn_gen_key = QPushButton("Генерировать"); btn_gen_key.setFixedWidth(110)
        btn_gen_key.clicked.connect(self._wsvr_generate_key)
        api_row.addWidget(self.ed_wsvr_api_key); api_row.addWidget(btn_eye_wsvr)
        api_row.addWidget(btn_gen_key); api_row.addStretch()
        cgl.addRow("API ключ:", api_row)

        wsvr_test_row = QHBoxLayout()
        btn_wsvr_test = QPushButton("📡 Проверить"); btn_wsvr_test.setFixedHeight(26)
        btn_wsvr_test.setToolTip("Проверить доступность сервера отчётов и корректность API-ключа "
                                 "(тестовый запрос на Report URL).")
        btn_wsvr_test.clicked.connect(self._test_report_server)
        self.lbl_wsvr_status = QLabel(""); self.lbl_wsvr_status.setWordWrap(True)
        wsvr_test_row.addWidget(btn_wsvr_test); wsvr_test_row.addWidget(self.lbl_wsvr_status, 1)
        cgl.addRow("", wsvr_test_row)

        lbl_hint = QLabel(
            "Агенты передают ключ в заголовке X-Api-Key. Все остальные настройки "
            "(порт, пользователи, Telegram, группы, история) — через веб-админку."
        )
        lbl_hint.setStyleSheet("color:gray;font-style:italic;font-size:8pt;"); lbl_hint.setWordWrap(True)
        cgl.addRow("", lbl_hint)
        ly.addWidget(cg)
        ly.addStretch()

        self._wsvr_load_config()

    def _build_updates_tab(self, tab=None):
        if tab is None:
            tab = QWidget()
        ly = QVBoxLayout(tab); ly.setSpacing(8)

        ug = QGroupBox("Обновления программы"); ugl = QFormLayout(ug)
        self.ed_update_url = QLineEdit(self.cfg.update_server_url)
        self.ed_update_url.setPlaceholderText("http://1221.by:3266")
        self.ed_update_url.textEdited.connect(self._changed)
        ugl.addRow("Сервер обновлений:", self.ed_update_url)

        upd_key_row = QHBoxLayout()
        self.ed_update_key = QLineEdit(self.cfg.update_api_key)
        self.ed_update_key.setPlaceholderText("Не нужен для общего сервера обновлений")
        self.ed_update_key.setFixedWidth(280)
        self.ed_update_key.setEchoMode(QLineEdit.Password)
        self.ed_update_key.editingFinished.connect(self._upd_key_restore_default)
        btn_show_upd_key = QToolButton(); btn_show_upd_key.setText("👁")
        btn_show_upd_key.setCheckable(True)
        btn_show_upd_key.toggled.connect(lambda v: self.ed_update_key.setEchoMode(
            QLineEdit.Normal if v else QLineEdit.Password))
        self._upd_key_eye = btn_show_upd_key
        btn_gen_upd_key = QPushButton("Генерировать"); btn_gen_upd_key.setFixedWidth(110)
        btn_gen_upd_key.clicked.connect(self._upd_generate_key)
        upd_key_row.addWidget(self.ed_update_key); upd_key_row.addWidget(btn_show_upd_key)
        btn_gen_upd_key.setVisible(False)  # ключ задаётся на сервере, не на клиенте
        upd_key_row.addWidget(btn_gen_upd_key); upd_key_row.addStretch()
        ugl.addRow("API ключ:", upd_key_row)

        upd_test_row = QHBoxLayout()
        btn_upd_test = QPushButton("📡 Проверить"); btn_upd_test.setFixedHeight(26)
        btn_upd_test.setToolTip("Проверить доступность сервера обновлений и корректность API-ключа "
                                "(GET /api/manifest).")
        btn_upd_test.clicked.connect(self._test_update_server)
        self.lbl_upd_test = QLabel(""); self.lbl_upd_test.setWordWrap(True)
        upd_test_row.addWidget(btn_upd_test); upd_test_row.addWidget(self.lbl_upd_test, 1)
        ugl.addRow("", upd_test_row)
        self._sync_encrypt_ui()

        self.chk_auto_update = QCheckBox("Автоматически загружать и применять обновления")
        self.chk_auto_update.setChecked(self.cfg.auto_update)
        ugl.addRow("", self.chk_auto_update)

        lbl_url_hint = QLabel("✓ Смена URL/ключа применяется агентом автоматически на ближайшем цикле "
                              "проверки — перезапуск службы не нужен. Кнопка «Проверить» сразу сохраняет ключ. "
                              "Ручная проверка снимает блокировку обновлений (circuit breaker).")
        lbl_url_hint.setStyleSheet("color:#15803d;font-size:8pt;font-style:italic;"); lbl_url_hint.setWordWrap(True)
        ugl.addRow("", lbl_url_hint)

        ly.addWidget(ug)

        rg = QGroupBox("REST API v1"); rgl = QFormLayout(rg)
        api_tok_row = QHBoxLayout()
        self.ed_api_token = QLineEdit(self.cfg.gg("api_token",""))
        self.ed_api_token.setFixedWidth(280)
        self.ed_api_token.setEchoMode(QLineEdit.Password)
        self.ed_api_token.setPlaceholderText("Если пусто — агент создаст токен сам")
        self.ed_api_token.textEdited.connect(self._changed)
        btn_eye_api = QToolButton(); btn_eye_api.setText("👁"); btn_eye_api.setCheckable(True)
        btn_eye_api.toggled.connect(lambda v: self.ed_api_token.setEchoMode(
            QLineEdit.Normal if v else QLineEdit.Password))
        api_tok_row.addWidget(self.ed_api_token); api_tok_row.addWidget(btn_eye_api); api_tok_row.addStretch()
        rgl.addRow("Bearer токен:", api_tok_row)
        lbl_api_hint = QLabel(
            "Токен для авторизации внешних запросов к /api/v1/*. "
            "Если оставить пустым — агент сам создаёт случайный токен при старте. "
            "Задайте в config.json (поле api_token) или здесь."
        )
        lbl_api_hint.setStyleSheet("color:gray;font-style:italic;font-size:8pt;"); lbl_api_hint.setWordWrap(True)
        rgl.addRow("", lbl_api_hint)
        ly.addWidget(rg)

        upd_btn_row = QHBoxLayout()
        self.btn_check_upd = QPushButton("🔄 Проверить и обновить"); self.btn_check_upd.setFixedHeight(26)
        self.btn_check_upd.clicked.connect(self._check_update_now)
        upd_btn_row.addWidget(self.btn_check_upd); upd_btn_row.addStretch()
        ugl.addRow("", upd_btn_row)

        self.lbl_gui_update_status = QLabel("GUI: —")
        self.lbl_gui_update_status.setStyleSheet("font-size:8pt;")
        ugl.addRow("Статус GUI:", self.lbl_gui_update_status)
        self.lbl_svc_update_status = QLabel("Service: —")
        self.lbl_svc_update_status.setStyleSheet("font-size:8pt;")
        ugl.addRow("Статус службы:", self.lbl_svc_update_status)
        # Backward compat: keep reference for _poll_update_status legacy path
        self.lbl_update_status = self.lbl_svc_update_status

        ly.addStretch()
        QTimer.singleShot(800, self._poll_update_status)

    def _upd_save_config(self):
        if hasattr(self, "ed_update_url"):
            self.cfg.update_server_url = self.ed_update_url.text().strip()
            self.cfg.update_api_key    = self.ed_update_key.text().strip()
            self.cfg.auto_update       = self.chk_auto_update.isChecked()
        if hasattr(self, "ed_api_token"):
            self.cfg.api_token = self.ed_api_token.text().strip()
        self.statusBar().showMessage("✅ Настройки обновлений сохранены", 4000)

    def _wsvr_load_config(self):
        cfg = WebServerManager.load_config()
        # load_config уже расшифровал; повторный _dpapi_decrypt — страховка, чтобы
        # enc:-блоб физически не мог оказаться в поле ввода (а оттуда — в
        # report_api_key и в заголовке X-Api-Key у всего парка агентов).
        api_key = _dpapi_decrypt(str(cfg.get("api_key", "") or ""))
        if api_key.startswith("enc:"):
            api_key = ""
            self.statusBar().showMessage(
                "⚠ API-ключ отчётов зашифрован на другой машине — расшифровать не удалось. "
                "Сгенерируйте новый ключ.", 10000)
        self.ed_wsvr_api_key.setText(api_key)
        self.ed_report_url.setText(self.cfg.report_url)
        if hasattr(self, "ed_update_url"):
            self.ed_update_url.setText(self.cfg.update_server_url)
            self.ed_update_key.setText(self.cfg.update_api_key)
            self.chk_auto_update.setChecked(self.cfg.auto_update)
        QTimer.singleShot(800, self._poll_update_status)

    def _wsvr_save_config(self):
        api_key = self.ed_wsvr_api_key.text().strip()
        if api_key.startswith("enc:"):
            # Последний рубеж: шифротекст не должен ни попасть в
            # webserver-config.json как «плейнтекстовый» ключ, ни уехать агентам
            # в report_api_key — они шлют его в X-Api-Key и получают 401.
            QMessageBox.warning(
                self, "Веб-сервер",
                "API-ключ выглядит как зашифрованное значение (enc:…) и не был "
                "расшифрован на этой машине.\n\nНастройки НЕ сохранены — "
                "сгенерируйте новый ключ кнопкой «Сгенерировать».")
            return
        cfg = WebServerManager.load_config()
        cfg.update({
            "api_key":          api_key,
        })
        WebServerManager.save_config(cfg)
        # sync api_key and report/update URLs to global config
        self.cfg.report_api_key = cfg["api_key"]
        self.cfg.report_url = self.ed_report_url.text().strip()
        if hasattr(self, "ed_update_url"):
            self.cfg.update_server_url = self.ed_update_url.text().strip()
            self.cfg.update_api_key    = self.ed_update_key.text().strip()
            self.cfg.auto_update       = self.chk_auto_update.isChecked()
        self.statusBar().showMessage("Настройки веб-сервера сохранены", 4000)

    def _wsvr_update_ui(self):
        """Опрос статуса webserver в фоне — sc query блокирует main thread на 15с."""
        cfg = WebServerManager.load_config()
        port = cfg.get("port", 8798)
        rport = cfg.get("report_port", 8797)
        import threading
        def _do():
            try:
                running = WebServerManager.is_running()
            except Exception:
                running = False
            def _ui():
                self._wsvr_running = running
                if running:
                    self.lbl_wsvr_status.setText(f"● Работает (UI:{port} / отчёты:{rport})")
                    self.lbl_wsvr_status.setStyleSheet("color:#006600;font-weight:bold;")
                else:
                    self.lbl_wsvr_status.setText("○ Остановлен")
                    self.lbl_wsvr_status.setStyleSheet("color:gray;")
            self._ui_call_signal.emit(_ui)
        threading.Thread(target=_do, daemon=True).start()

    def _check_update_now(self):
        if hasattr(self, "btn_check_upd"):
            self.btn_check_upd.setEnabled(False)
            self.btn_check_upd.setText("Проверка...")
        if hasattr(self, "lbl_gui_update_status"):
            self.lbl_gui_update_status.setText("Проверка...")
        if hasattr(self, "lbl_svc_update_status"):
            self.lbl_svc_update_status.setText("Проверка...")
        try:
            import datetime as _dt
            self._upd_check_started = _dt.datetime.now(_dt.timezone.utc)
            # через agent_api_request — иначе нет Bearer и агент отдаёт 401
            agent_api_request("/update/check", method="POST", payload={}, timeout=3)
            QTimer.singleShot(2000, self._poll_for_update_dialog)
        except Exception:
            if hasattr(self, "lbl_gui_update_status"):
                self.lbl_gui_update_status.setText("Агент недоступен")
            if hasattr(self, "lbl_svc_update_status"):
                self.lbl_svc_update_status.setText("Агент недоступен")
            self._upd_unlock_check_btn()

    def _poll_for_update_dialog(self, attempt: int = 0):
        """Ждём завершения проверки версий, затем показываем диалог YES/NO."""
        try:
            d = agent_api_request("/update/status", timeout=3)
            if not isinstance(d, dict):
                raise RuntimeError("некорректный ответ /update/status")
            phase = d.get("phase", "")
            svc_cur    = d.get("current_version", "?")
            svc_latest = d.get("latest_version", "") or ""
            gui_cur    = d.get("gui_version", "?")
            gui_latest = d.get("gui_latest_version", "") or ""
            # Ждём пока last_check агента не окажется ПОСЛЕ момента нашего нажатия
            import datetime as _dt
            last_check_str = d.get("last_check") or ""
            check_is_fresh = False
            if last_check_str:
                try:
                    lc = _dt.datetime.fromisoformat(last_check_str)
                    if lc.tzinfo is None:
                        lc = lc.replace(tzinfo=_dt.timezone.utc)
                    started = getattr(self, "_upd_check_started", None)
                    check_is_fresh = started is None or lc >= started
                except Exception:
                    check_is_fresh = False
            still_waiting = phase == "checking" or not check_is_fresh
            if still_waiting and attempt < 10:
                QTimer.singleShot(2000, lambda: self._poll_for_update_dialog(attempt + 1))
                return
            last = (d.get("last_check") or "")[:16].replace("T", " ")
            ts_tail = f"  |  Проверено: {last}" if last else ""
            has_gui_upd = bool(gui_latest) and gui_cur != gui_latest
            has_svc_upd = bool(svc_latest) and svc_cur != svc_latest
            gui_status = f"Доступно: {gui_latest}" if has_gui_upd else "Актуально"
            svc_status = f"Доступно: {svc_latest}" if has_svc_upd else "Актуально"
            if hasattr(self, "lbl_gui_update_status"):
                self.lbl_gui_update_status.setText(
                    f"текущая: {gui_cur}  |  доступна: {gui_latest or '—'}{ts_tail}  |  {gui_status}")
            if hasattr(self, "lbl_svc_update_status"):
                self.lbl_svc_update_status.setText(
                    f"текущая: {svc_cur}  |  доступна: {svc_latest or '—'}{ts_tail}  |  {svc_status}")
            if not has_gui_upd and not has_svc_upd:
                self._upd_unlock_check_btn()
                return
            lines = []
            if has_gui_upd: lines.append(f"  • GUI: {gui_cur} → {gui_latest}")
            if has_svc_upd: lines.append(f"  • Служба: {svc_cur} → {svc_latest}")
            reply = QMessageBox.question(
                self, "Обновление RestLex",
                "Доступны обновления:\n" + "\n".join(lines) + "\n\nПрименить сейчас?",
                QMessageBox.Yes | QMessageBox.No)
            if reply != QMessageBox.Yes:
                self._upd_unlock_check_btn()
                return
            agent_api_request("/update/apply", method="POST", payload={}, timeout=3)
            if has_gui_upd:
                self.statusBar().showMessage("🔄 Обновление GUI — перезапуск...", 0)
                if hasattr(self, "btn_check_upd"):
                    self.btn_check_upd.setText("Перезапуск...")
                QTimer.singleShot(1500, lambda: QApplication.instance().quit())
            else:
                QTimer.singleShot(2000, lambda: self._poll_update_status(0))
        except Exception:
            if hasattr(self, "lbl_gui_update_status"):
                self.lbl_gui_update_status.setText("Агент недоступен")
            if hasattr(self, "lbl_svc_update_status"):
                self.lbl_svc_update_status.setText("Агент недоступен")
            self._upd_unlock_check_btn()

    def _poll_update_status(self, attempt: int = 0):
        if not (hasattr(self, "lbl_gui_update_status") or hasattr(self, "lbl_update_status")):
            return
        import threading
        def _fetch():
            try:
                d = agent_api_request("/update/status", timeout=3)
                if not isinstance(d, dict):
                    raise RuntimeError("некорректный ответ /update/status")
                self._ui_call_signal.emit(lambda: self._apply_update_status(d, attempt))
            except Exception:
                self._ui_call_signal.emit(lambda: self._handle_update_status_error(attempt))
        threading.Thread(target=_fetch, daemon=True).start()

    def _apply_update_status(self, d: dict, attempt: int):
        phase = d.get("phase", "")
        svc_cur    = d.get("current_version", "?")
        svc_latest = d.get("latest_version", "") or "—"
        gui_cur    = d.get("gui_version", "?")
        gui_latest = d.get("gui_latest_version", "") or "—"
        last = (d.get("last_check") or "")[:16].replace("T", " ")
        phase_ru = {
            "idle": "—", "checking": "Проверка...",
            "downloading": "Загрузка...", "waiting_backup": "Ожидание бекапа",
            "applying": "Установка...", "up_to_date": "Актуально",
        }.get(phase, phase)
        ts_tail = f"  |  Проверено: {last}" if last else ""
        if phase in ("checking", "downloading", "applying", "waiting_backup"):
            gui_phase_ru = phase_ru
        elif gui_latest != "—" and gui_cur != gui_latest:
            gui_phase_ru = "Доступно обновление"
        else:
            gui_phase_ru = "Актуально"
        if phase in ("checking", "downloading", "applying", "waiting_backup"):
            svc_phase_ru = phase_ru
        elif svc_latest != "—" and svc_cur != svc_latest:
            svc_phase_ru = "Доступно обновление"
        else:
            svc_phase_ru = phase_ru if phase_ru != "—" else "Актуально"
        gui_tail = f"  |  {gui_phase_ru}" + ts_tail
        svc_tail = f"  |  {svc_phase_ru}" + ts_tail
        if hasattr(self, "lbl_gui_update_status"):
            self.lbl_gui_update_status.setText(f"текущая: {gui_cur}  |  доступна: {gui_latest}{gui_tail}")
        if hasattr(self, "lbl_svc_update_status"):
            self.lbl_svc_update_status.setText(f"текущая: {svc_cur}  |  доступна: {svc_latest}{svc_tail}")
        active = phase in ("checking", "downloading", "applying", "waiting_backup")
        if active:
            QTimer.singleShot(2000, lambda: self._poll_update_status(attempt + 1))
        else:
            self._upd_unlock_check_btn()

    def _handle_update_status_error(self, attempt: int):
        if attempt < 30:
            msg = f"Перезапуск службы... ({attempt * 2}с)"
            if hasattr(self, "lbl_gui_update_status"):
                self.lbl_gui_update_status.setText(msg)
            if hasattr(self, "lbl_svc_update_status"):
                self.lbl_svc_update_status.setText(msg)
            QTimer.singleShot(2000, lambda: self._poll_update_status(attempt + 1))
        else:
            if hasattr(self, "lbl_gui_update_status"):
                self.lbl_gui_update_status.setText("Агент недоступен")
            if hasattr(self, "lbl_svc_update_status"):
                self.lbl_svc_update_status.setText("Агент недоступен")
            self._upd_unlock_check_btn()

    def _upd_unlock_check_btn(self):
        if hasattr(self, "btn_check_upd"):
            self.btn_check_upd.setEnabled(True)
            self.btn_check_upd.setText("🔄 Проверить и обновить")

    def _wsvr_btn_clicked(self):
        self._wsvr_save_config()
        cfg = WebServerManager.load_config()
        if WebServerManager.is_running():
            ok, msg = WebServerManager.stop()
        else:
            ok, msg = WebServerManager.start(cfg)
            if not ok and ("не установлена" in msg.lower() or "1060" in msg):
                QMessageBox.warning(self, "Веб-сервер",
                    "Служба RestlexWebServer не установлена.\n\n"
                    "Установите через restlex-webserver.exe install --api-key КЛЮЧ --admin-password ПАРОЛЬ")
        self._wsvr_update_ui()
        self.statusBar().showMessage(("● " if ok else "✗ ") + msg, 6000)

    def _wsvr_generate_key(self):
        import secrets as _sec
        self.ed_wsvr_api_key.setText(_sec.token_hex(24))
        self._wsvr_save_config()
        self.statusBar().showMessage("🔑 Новый API-ключ отчётов сгенерирован и сохранён", 4000)

    def _upd_generate_key(self):
        import secrets as _sec
        self.ed_update_key.setText(_sec.token_hex(24))
        self._upd_save_config()
        self.statusBar().showMessage("🔑 Новый API-ключ обновлений сгенерирован и сохранён", 4000)

    def _upd_key_restore_default(self):
        if not self.ed_update_key.text().strip():
            self.ed_update_key.setText(Config._UPD_API_KEY_DEFAULT)

    def _wsvr_open_browser(self):
        import webbrowser
        port = WebServerManager.load_config().get("port", 8798)
        webbrowser.open(f"http://127.0.0.1:{port}")

    def _wsvr_clear_db(self):
        if QMessageBox.question(
            self, "Очистить БД",
            "Удалить все отчёты из базы данных веб-сервера?\n\n"
            "Это действие необратимо.",
            QMessageBox.Yes | QMessageBox.No
        ) != QMessageBox.Yes:
            return
        db_path = PROGRAMDATA_ROOT / APP_DATA_DIRNAME / "webserver.db"
        try:
            if db_path.exists():
                db_path.unlink()
            self.statusBar().showMessage("🗑 БД отчётов очищена", 5000)
        except Exception as e:
            QMessageBox.warning(self, "Ошибка", f"Не удалось удалить БД:\n{e}")

    def _build_web_tab(self):
        """Создаёт закладку 🌐 WEB с двумя подзакладками: Restic-Server и Веб сервер отчётов."""
        outer = QWidget()
        outer_ly = QVBoxLayout(outer); outer_ly.setContentsMargins(0, 4, 0, 0); outer_ly.setSpacing(0)
        inner = QTabWidget()
        outer_ly.addWidget(inner)

        t_rsvr = QWidget()
        self._build_rest_server_tab(t_rsvr)
        inner.addTab(t_rsvr, "Restic-Server")

        t_wsvr = QWidget()
        self._build_webserver_tab(t_wsvr)
        inner.addTab(t_wsvr, "Веб сервер отчётов")

        t_upd = QWidget()
        self._build_updates_tab(t_upd)
        inner.addTab(t_upd, "⬇ Обновления")

        self._tab_web = outer
        self.tabs.addTab(outer, "🌐 Веб")

    def _sched_toggle_ui(self):
        btn = self._sched_bg.checkedButton()
        if not btn: return
        freq = btn.property("freq")
        self._sched_time_row.setVisible(freq in ("daily","weekly"))
        self._sched_days_row.setVisible(freq == "weekly")
        self._sched_interval_row.setVisible(freq == "hourly")

    def _sched_day_label(self, code: str) -> str:
        return {
            "MON": "Пн",
            "TUE": "Вт",
            "WED": "Ср",
            "THU": "Чт",
            "FRI": "Пт",
            "SAT": "Сб",
            "SUN": "Вс",
        }.get(str(code or "").upper(), str(code or ""))

    def _sched_method_label(self, sched: dict) -> str:
        freq = str((sched or {}).get("freq", "daily") or "daily")
        return {
            "daily": "Ежедневно",
            "weekly": "Еженедельно",
            "hourly": "Каждые N часов",
            "onlogon": "При входе",
        }.get(freq, freq)

    def _sched_when_label(self, sched: dict) -> str:
        sched = sched or {}
        freq = str(sched.get("freq", "daily") or "daily")
        times_list = sched.get("times") or [sched.get("time", "02:00")]
        times_str = ", ".join(sorted(set(str(t) for t in times_list if t)))
        if freq == "daily":
            return f"Каждый день в {times_str}"
        if freq == "weekly":
            days = [self._sched_day_label(code) for code in str(sched.get("days", "MON")).split(",") if code]
            return f"{', '.join(days) or 'Пн'} в {times_str}"
        if freq == "hourly":
            return f"Каждые {int(sched.get('interval', 4) or 4)} ч"
        if freq == "onlogon":
            return "При входе пользователя"
        return "—"

    def _sched_normalize_entry(self, sched: Optional[dict], task_id: str = "", legacy: bool = False) -> dict:
        sched = dict(sched or {})
        run_as = str(sched.get("run_as", "system") or "system").strip().lower()
        base_time = str(sched.get("time", "02:00") or "02:00")
        raw_times = sched.get("times") or [base_time]
        times = sorted(set(str(t) for t in raw_times if t)) or [base_time]
        return {
            "task_id": task_id or str(sched.get("task_id") or "").strip(),
            "_legacy": bool(legacy or sched.get("_legacy")),
            "freq": str(sched.get("freq", "daily") or "daily"),
            "time": times[0],
            "times": times,
            "days": str(sched.get("days", "MON") or "MON"),
            "interval": int(sched.get("interval", 4) or 4),
            "forget": bool(sched.get("forget", False)),
            "notify_tg": bool(sched.get("notify_tg", False)),
            "run_as": run_as if run_as in ("current", "system") else "system",
            "params": dict(sched.get("params") or {}),
        }

    def _sched_profile_entries(self, prof: Optional[dict] = None) -> List[dict]:
        prof = prof or (self.cfg.profile(self.pid) or {})
        sched = prof.get("sched") or {}
        if not sched:
            return []
        return [self._sched_normalize_entry(sched, task_id="main", legacy=False)]

    def _sched_store_entries(self, prof: dict, entries: List[dict]):
        prof.pop("sched_tasks", None)
        if not entries:
            prof.pop("sched", None)
            return
        entry = entries[0]
        times = entry.get("times") or [entry.get("time", "02:00")]
        # enabled — владение агента/Recondeck (пауза профиля через API v1).
        # GUI его не показывает, поэтому обязан сохранить как есть: иначе любое
        # сохранение профиля молча снимало бы паузу, поставленную в панели.
        # merge, а не пересборка с нуля: любое поле, которого GUI не знает
        # (jitter, window, misfire-policy и прочее от агента/Recondeck), раньше
        # молча выбрасывалось при сохранении задачи.
        merged = dict(prof.get("sched") or {})
        prev_enabled = merged.get("enabled", True)
        merged.update({
            "enabled": bool(prev_enabled),
            "freq": entry.get("freq", "daily"),
            "time": entry.get("time", "02:00"),
            "times": sorted(set(str(t) for t in times if t)) or ["02:00"],
            "days": entry.get("days", "MON"),
            "interval": int(entry.get("interval", 4) or 4),
            "forget": bool(entry.get("forget", False)),
            "notify_tg": bool(entry.get("notify_tg", False)),
            "run_as": entry.get("run_as", "system"),
            "params": dict(entry.get("params") or {}),
        })
        prof["sched"] = merged

    def _sched_entry_signature(self, sched: dict) -> str:
        entry = self._sched_normalize_entry(sched)
        entry.pop("task_id", None)
        entry.pop("_legacy", None)
        return json.dumps(entry, sort_keys=True, ensure_ascii=False)

    def _sched_selected_key(self):
        item = getattr(self, "sched_tasks", None).currentItem() if hasattr(self, "sched_tasks") else None
        if not item:
            return "", False
        return str(item.data(0, Qt.UserRole) or ""), bool(item.data(0, Qt.UserRole + 1))

    def _sched_find_entry_by_key(self, entries: List[dict], task_id: str, legacy: bool) -> Optional[dict]:
        for entry in entries:
            if bool(entry.get("_legacy")) == legacy and str(entry.get("task_id") or "") == str(task_id or ""):
                return entry
        return None

    def _sched_load_form_from_entry(self, entry: Optional[dict]):
        if not entry:
            return
        self._sched_params_user_opened = False
        self._sched_loading = True
        try:
            freq = str(entry.get("freq", "daily") or "daily")
            for btn in self._sched_bg.buttons():
                btn.setChecked(btn.property("freq") == freq)
            raw_times = entry.get("times") or [str(entry.get("time", "02:00") or "02:00")]
            times = sorted(set(raw_times)) or ["02:00"]
            self.sched_times_list.clear()
            for tv in times:
                self.sched_times_list.addItem(tv)
            t = QTime.fromString(times[0], "HH:mm")
            if t.isValid():
                self.sched_time.setTime(t)
            days = str(entry.get("days", "MON") or "MON").split(",")
            for code, cb in self._day_chks.items():
                cb.setChecked(code in days)
            self.sched_interval.setValue(int(entry.get("interval", 4) or 4))
            self.chk_sched_forget.setChecked(bool(entry.get("forget", False)))
            self.chk_sched_tg.setChecked(bool(entry.get("notify_tg", False)))
            run_as = str(entry.get("run_as", "current") or "current").strip().lower()
            for btn in self._sched_user_bg.buttons():
                btn.setChecked(btn.property("run_as") == run_as)
            self._sched_toggle_ui()
            self._sched_overlay_apply(entry)
            # Запоминаем, какая задача сейчас в форме, чтобы повторный выбор
            # той же задачи не перезагружал форму и не терял правки.
            self._sched_loaded_key = (self.pid, str((entry or {}).get("task_id") or ""))
        finally:
            self._sched_loading = False

    def _refresh_sched_retention_label(self):
        lbl = getattr(self, "lbl_sched_retention", None)
        if lbl is None:
            return
        if not self.pid:
            lbl.setVisible(False)
            return
        prof = self.cfg.profile(self.pid) or {}
        ret = prof.get("retention", {}) or {}
        active = [(k, int(v)) for k, v in ret.items() if v]
        if not active:
            lbl.setText("⚠ Правила хранения не заданы — prune ничего не оставит, очистка отключена")
            lbl.setStyleSheet("color:#b94a48;font-size:9pt;padding-left:20px;")
            lbl.setVisible(True)
            return
        ru = {
            "keep-last":    ("последних копий",     "последних копий"),
            "keep-hourly":  ("за последний час",    "за последние {n} часов"),
            "keep-daily":   ("за последний день",   "за последние {n} дней"),
            "keep-weekly":  ("за последнюю неделю", "за последние {n} недель"),
            "keep-monthly": ("за последний месяц",  "за последние {n} месяцев"),
            "keep-yearly":  ("за последний год",    "за последние {n} лет"),
        }
        parts = []
        for k, v in active:
            one, many = ru.get(k, (k, k + " ({n})"))
            label = one if v == 1 else many.format(n=v)
            parts.append(f"{v} {label}" if v != 1 else f"1 {label}")
        text = "Сохраняем: " + "; ".join(parts)
        if self.chk_sched_forget.isChecked():
            text += "  — остальное удаляется после бэкапа"
        else:
            text += "  — очистка отключена, снапшоты накапливаются"
        lbl.setText(text)
        lbl.setStyleSheet("color:#555;font-size:9pt;padding-left:20px;")
        lbl.setVisible(True)

    def _sched_on_selection_changed(self):
        self._sched_update_action_buttons()
        if self._sched_loading or not self.pid:
            return
        task_id, legacy = self._sched_selected_key()
        # Если выбрана та же задача того же профиля, что уже загружена в форму —
        # НЕ перезагружаем форму, иначе несохранённые правки пользователя
        # затираются значениями с диска (баг «Сохранить не сохраняет»).
        if (self.pid, str(task_id or "")) == getattr(self, "_sched_loaded_key", None):
            return
        prof = self.cfg.profile(self.pid) or {}
        entry = self._sched_find_entry_by_key(self._sched_profile_entries(prof), task_id, legacy)
        if entry:
            self._sched_load_form_from_entry(entry)

    def _sched_update_action_buttons(self):
        task_id, _legacy = self._sched_selected_key()
        has_task = bool(task_id)
        has_profile = bool(getattr(self, "pid", None))
        if hasattr(self, "btn_sched_update"):
            self.btn_sched_update.setEnabled(has_task)
        if hasattr(self, "btn_sched_run"):
            self.btn_sched_run.setEnabled(has_profile)
        if hasattr(self, "btn_sched_del"):
            self.btn_sched_del.setEnabled(has_task)

    def _sched_refresh_manager(self, select_key=None):
        if not hasattr(self, "sched_tasks"):
            return []
        current_key = select_key or self._sched_selected_key()
        self._sched_loading = True
        try:
            self.sched_tasks.blockSignals(True)
            if not self.pid:
                self.sched_tasks.clear()
                self._sched_update_action_buttons()
                return []
            prof = self.cfg.profile(self.pid) or {}
            entries = self._sched_profile_entries(prof)
            # Если число задач совпадает — обновляем существующие строки на месте,
            # чтобы не воровать фокус у редактируемых полей (Итоговая команда).
            existing = self.sched_tasks.topLevelItemCount()
            in_place = (existing == len(entries) and existing > 0)
            if not in_place:
                self.sched_tasks.clear()
            # Fetch agent last-run once for the whole profile (shared across all tasks)
            agent_st = SchedulerManager.status(self.pid, "")
            lr = agent_st.get("last_run") or {}
            last_run_ts = lr.get("timestamp", "") if isinstance(lr, dict) else ""
            selected_item = None
            cols_status = agent_st.get("status", "Нет данных")
            for idx, entry in enumerate(entries, 1):
                task_name = f"Задача {idx}"
                run_as_lbl = "SYSTEM" if entry.get("run_as") == "system" else "Текущий"
                next_fire = SchedulerManager._compute_next_fire(entry, last_run_ts)
                cols = [
                    task_name,
                    self._sched_method_label(entry),
                    self._sched_when_label(entry),
                    self._sched_prune_label(entry),
                    "Да" if entry.get("notify_tg") else "Нет",
                    run_as_lbl,
                    self._sched_overlay_short_label(entry),
                    next_fire,
                    cols_status,
                ]
                cols = [i18n.tr(c) for c in cols]  # дерево не проходит через перехваченные сеттеры
                if in_place:
                    item = self.sched_tasks.topLevelItem(idx - 1)
                    for ci, val in enumerate(cols):
                        if item.text(ci) != val:
                            item.setText(ci, val)
                else:
                    item = QTreeWidgetItem(cols)
                    item.setData(0, Qt.UserRole, str(entry.get("task_id") or ""))
                    item.setData(0, Qt.UserRole + 1, False)
                    item.setToolTip(0, task_name)
                    self.sched_tasks.addTopLevelItem(item)
                if current_key and current_key == (str(entry.get("task_id") or ""), False):
                    selected_item = item
            if selected_item is None and self.sched_tasks.topLevelItemCount():
                selected_item = self.sched_tasks.topLevelItem(0)
            if selected_item is not None and self.sched_tasks.currentItem() is not selected_item:
                self.sched_tasks.setCurrentItem(selected_item)
            self._sched_update_action_buttons()
            return entries
        finally:
            self.sched_tasks.blockSignals(False)
            self._sched_loading = False

    def _sched_collect(self) -> dict:
        btn  = self._sched_bg.checkedButton()
        freq = btn.property("freq") if btn else "daily"
        days = ",".join(c for c, w in self._day_chks.items() if w.isChecked()) or "MON"
        user_btn = self._sched_user_bg.checkedButton()
        run_as = user_btn.property("run_as") if user_btn else "current"
        times = sorted({self.sched_times_list.item(i).text()
                        for i in range(self.sched_times_list.count())} or {"02:00"})
        return {
            "freq":             freq,
            "time":             times[0],
            "times":            times,
            "days":             days,
            "interval":         self.sched_interval.value(),
            "forget":           self.chk_sched_forget.isChecked(),
            "notify_tg":        self.chk_sched_tg.isChecked(),
            "run_as":           run_as,
            "params":           self._sched_overlay_collect() if (hasattr(self, "_sched_params_group") and self._sched_params_group.isChecked()) else {},
        }

    # ── Overlay параметров запуска ───────────────────────────────────────
    def _sched_overlay_defaults(self) -> dict:
        prof = self.cfg.profile(self.pid) or {}
        return {
            "no_scan":             bool(self.cfg.global_no_scan),
            "use_vss":             bool(prof.get("use_vss", True)),
            "pre_unlock":          bool(prof.get("pre_unlock", True)),
            "force_unlock_orphan": bool(prof.get("force_unlock_orphan", True)),
            "pre_repair_index":    bool(prof.get("pre_repair_index", True)),
            "stale_grace_minutes": int(prof.get("stale_grace_minutes", 35) or 35),
        }

    def _sched_overlay_collect(self) -> dict:
        defaults = self._sched_overlay_defaults()
        params = {}
        ui_simple = {
            "pre_unlock":          self.chk_p_pre_unlock.isChecked(),
            "force_unlock_orphan": self.chk_p_force_orphan.isChecked(),
            "pre_repair_index":    self.chk_p_pre_repair.isChecked(),
            "stale_grace_minutes": int(self.spin_p_stale_grace.value()),
        }
        for k, v in ui_simple.items():
            if v != defaults.get(k):
                params[k] = v
        cli = self.ed_p_preview.toPlainText().strip()
        if cli:
            parsed = self._sched_parse_preview_cmd(cli)
            if parsed is not None:
                if parsed["no_scan"] != defaults["no_scan"]:
                    params["no_scan"] = parsed["no_scan"]
                if parsed["use_vss"] != defaults["use_vss"]:
                    params["use_vss"] = parsed["use_vss"]
                prof_exc = set(self.cfg.profile(self.pid).get("exclude", []) if self.pid else [])
                extra_exc = [e for e in parsed["exclude"] if e not in prof_exc]
                if extra_exc:
                    params["exclude"] = extra_exc
                if parsed["extra_args"]:
                    params["extra_args"] = parsed["extra_args"]
        else:
            if self.chk_p_no_scan.isChecked() != defaults["no_scan"]:
                params["no_scan"] = self.chk_p_no_scan.isChecked()
            if self.chk_p_use_vss.isChecked() != defaults["use_vss"]:
                params["use_vss"] = self.chk_p_use_vss.isChecked()
        # Forget overlay: парсим retention из forget-превью если он отличается от профиля
        fg = self.ed_p_preview_forget.toPlainText().strip() if hasattr(self, "ed_p_preview_forget") else ""
        retention_overlay = self._sched_parse_forget_cmd(fg)
        if retention_overlay is not None:
            prof_ret = (self.cfg.profile(self.pid) or {}).get("retention", {}) or {}
            diff = {}
            for k in ("keep-last","keep-hourly","keep-daily","keep-weekly","keep-monthly","keep-yearly"):
                v_overlay = int(retention_overlay.get(k, 0) or 0)
                v_prof    = int(prof_ret.get(k, 0) or 0)
                if v_overlay != v_prof:
                    diff[k] = v_overlay
            if diff:
                params["retention"] = diff
        return params

    def _sched_parse_forget_cmd(self, cli: str):
        if not cli or cli.lstrip().startswith("#"):
            return None
        try:
            import shlex
            tokens = shlex.split(cli.replace("\n", " "), posix=False)
        except Exception:
            return None
        if "forget" in tokens:
            idx = tokens.index("forget")
            tokens = tokens[idx + 1:]
        ret = {}
        i = 0
        while i < len(tokens):
            t = tokens[i].strip('"')
            if t in ("--keep-last","--keep-hourly","--keep-daily",
                     "--keep-weekly","--keep-monthly","--keep-yearly") and i + 1 < len(tokens):
                try:
                    ret[t[2:]] = int(tokens[i + 1].strip('"'))
                except Exception:
                    pass
                i += 1
            i += 1
        return ret

    def _sched_overlay_apply(self, entry: dict):
        defaults = self._sched_overlay_defaults()
        p = (entry or {}).get("params", {}) or {}
        self._sched_pending_retention = dict(p.get("retention") or {})
        def _get(k, default):
            return p[k] if k in p else default
        self.chk_p_no_scan.setChecked(bool(_get("no_scan", defaults["no_scan"])))
        self.chk_p_use_vss.setChecked(bool(_get("use_vss", defaults["use_vss"])))
        self.chk_p_pre_unlock.setChecked(bool(_get("pre_unlock", defaults["pre_unlock"])))
        self.chk_p_force_orphan.setChecked(bool(_get("force_unlock_orphan", defaults["force_unlock_orphan"])))
        self.chk_p_pre_repair.setChecked(bool(_get("pre_repair_index", defaults["pre_repair_index"])))
        self.spin_p_stale_grace.setValue(int(_get("stale_grace_minutes", defaults["stale_grace_minutes"])))
        # Открывать группу ТОЛЬКО если пользователь сам поставил галку вручную.
        # Сохранённые params загружаются в виджеты, но группа остаётся закрытой по умолчанию.
        if hasattr(self, "_sched_params_group"):
            self._sched_params_group.blockSignals(True)
            try:
                open_group = getattr(self, "_sched_params_user_opened", False)
                self._sched_params_group.setChecked(open_group)
            finally:
                self._sched_params_group.blockSignals(False)
            for ch in self._sched_params_group.children():
                if isinstance(ch, QWidget) and ch is not self._sched_params_group:
                    ch.setVisible(open_group)
        # force=True — при выборе задачи всегда строим CLI заново из params,
        # игнорируя возможный мусор от предыдущего рендера
        self._sched_render_preview_cmd(force=True)

    def _refresh_prune_column(self):
        if not hasattr(self, "sched_tasks") or not self.pid:
            return
        prof = self.cfg.profile(self.pid) or {}
        entries = self._sched_profile_entries(prof)
        for i in range(self.sched_tasks.topLevelItemCount()):
            item = self.sched_tasks.topLevelItem(i)
            if i < len(entries):
                # _autosave ещё не сохранил текущие значения retention, читаем из UI
                ret_ui = {k: sp.value() for k, sp in self._sp.items() if sp.value()}
                fake_entry = dict(entries[i])
                # подменяем prof.retention для расчёта лейбла
                if fake_entry.get("forget"):
                    abbr = [("keep-last","L"),("keep-hourly","H"),("keep-daily","D"),
                            ("keep-weekly","W"),("keep-monthly","M"),("keep-yearly","Y")]
                    parts = [f"{a}={ret_ui[k]}" for k, a in abbr if ret_ui.get(k, 0) > 0]
                    item.setText(3, ", ".join(parts) if parts else "⚠ нет правил")
                else:
                    item.setText(3, "Нет")

    def _sched_prune_label(self, entry: dict) -> str:
        if not entry.get("forget"):
            return "Нет"
        prof = self.cfg.profile(self.pid) or {}
        ret = dict(prof.get("retention", {}) or {})
        ret.update((entry.get("params") or {}).get("retention") or {})
        abbr = [("keep-last", "L"), ("keep-hourly", "H"), ("keep-daily", "D"),
                ("keep-weekly", "W"), ("keep-monthly", "M"), ("keep-yearly", "Y")]
        parts = []
        for k, a in abbr:
            v = int(ret.get(k, 0) or 0)
            if v > 0:
                parts.append(f"{a}={v}")
        return ", ".join(parts) if parts else "⚠ нет правил"

    def _sched_overlay_short_label(self, entry: dict) -> str:
        p = (entry or {}).get("params") or {}
        if not p:
            return "(наследует)"
        bits = []
        if "no_scan" in p:
            bits.append("no-scan" if p["no_scan"] else "scan")
        if "use_vss" in p:
            bits.append("vss" if p["use_vss"] else "no-vss")
        if "pre_unlock" in p and not p["pre_unlock"]:
            bits.append("no-preflight")
        if "force_unlock_orphan" in p and not p["force_unlock_orphan"]:
            bits.append("no-force-unlock")
        if p.get("exclude"):
            bits.append(f"+{len(p['exclude'])} excl")
        if p.get("extra_args"):
            bits.append(f"+{len(p['extra_args'])} args")
        return ", ".join(bits) if bits else "(custom)"

    def _sched_reset_overlay(self):
        defaults = self._sched_overlay_defaults()
        self.chk_p_no_scan.setChecked(defaults["no_scan"])
        self.chk_p_use_vss.setChecked(defaults["use_vss"])
        self.chk_p_pre_unlock.setChecked(defaults["pre_unlock"])
        self.chk_p_force_orphan.setChecked(defaults["force_unlock_orphan"])
        self.chk_p_pre_repair.setChecked(defaults["pre_repair_index"])
        self.spin_p_stale_grace.setValue(defaults["stale_grace_minutes"])
        self._sched_pending_retention = {}
        self._sched_render_preview_cmd(force=True)

    def _sched_render_preview_cmd(self, force: bool = False):
        if not hasattr(self, "ed_p_preview"):
            return
        # Не затирать CLI пока пользователь активно его редактирует
        if not force and self.ed_p_preview.hasFocus():
            return
        prof = self.cfg.profile(self.pid) or {}
        sources = prof.get("sources", []) or []
        excludes = list(prof.get("exclude", []) or [])
        # Сохранить ручные правки exclude/extra_args, если они уже есть в превью
        cur_cli = self.ed_p_preview.toPlainText().strip()
        parsed_prev = self._sched_parse_preview_cmd(cur_cli) if cur_cli else None
        prof_exc_set = set(excludes)
        kept_extra_exc = []
        kept_extra_args = []
        if parsed_prev and not force:
            kept_extra_exc = [e for e in parsed_prev.get("exclude", []) if e not in prof_exc_set]
            kept_extra_args = list(parsed_prev.get("extra_args", []) or [])
        excludes += kept_extra_exc
        extra_args = kept_extra_args
        parts = ["restic.exe backup --json"]
        if self.chk_p_use_vss.isChecked():
            parts.append("--use-fs-snapshot")
        if self.chk_p_no_scan.isChecked():
            parts.append("--no-scan")
        for e in excludes:
            parts.append(f'--exclude "{e}"')
        for a in extra_args:
            parts.append(a)
        for s in sources:
            parts.append(f'"{s}"')
        text = "\n  ".join(parts)
        # Блокируем сигнал чтобы не зациклить
        self.ed_p_preview.blockSignals(True)
        try:
            self.ed_p_preview.setPlainText(text)
        finally:
            self.ed_p_preview.blockSignals(False)
        # Отдельная превью для forget --prune (редактируемая, перетягиваем правки)
        if hasattr(self, "ed_p_preview_forget"):
            if not force and self.ed_p_preview_forget.hasFocus():
                return  # не затирать пока пользователь правит
            if hasattr(self, "chk_sched_forget") and self.chk_sched_forget.isChecked():
                # Приоритет: текущие правки в превью → overlay из _sched_pending_retention → профиль
                ret_overlay = getattr(self, "_sched_pending_retention", None)
                ret_eff = dict(prof.get("retention", {}) or {})
                if ret_overlay:
                    ret_eff.update(ret_overlay)
                keep_flags = []
                for k in ("keep-last", "keep-hourly", "keep-daily",
                          "keep-weekly", "keep-monthly", "keep-yearly"):
                    v = int(ret_eff.get(k, 0) or 0)
                    if v > 0:
                        keep_flags.append(f"--{k} {v}")
                if keep_flags:
                    fg_text = "restic.exe forget --prune --json\n  " + "\n  ".join(keep_flags)
                else:
                    fg_text = "# ⚠ Правила хранения не заданы — forget не запустится"
            else:
                fg_text = "# Очистка отключена — forget не запускается"
            self.ed_p_preview_forget.blockSignals(True)
            try:
                self.ed_p_preview_forget.setPlainText(fg_text)
            finally:
                self.ed_p_preview_forget.blockSignals(False)

    def _sched_parse_preview_cmd(self, cli: str):
        """Разбор отредактированной CLI-строки обратно в dict.
        Возвращает {'no_scan':bool, 'use_vss':bool, 'exclude':[..], 'extra_args':[..]} или None.
        Парсим ТОЛЬКО блок 'restic ... backup ...' до начала второй команды/комментария."""
        if not cli:
            return None
        # Берём только до '# После бэкапа' / 'restic.exe forget' / 'restic forget'
        cut_markers = ["\n# ", "\nrestic.exe forget", "\nrestic forget"]
        backup_block = cli
        for m in cut_markers:
            idx = backup_block.find(m)
            if idx >= 0:
                backup_block = backup_block[:idx]
        try:
            import shlex
            tokens = shlex.split(backup_block.replace("\n", " "), posix=False)
        except Exception:
            return None
        if "backup" in tokens:
            idx = tokens.index("backup")
            tokens = tokens[idx + 1:]
        no_scan = False
        use_vss = False
        excludes = []
        extra = []
        i = 0
        known_with_arg = {"--exclude", "-e", "--exclude-file", "--read-concurrency",
                          "--compression", "--pack-size", "--limit-upload", "--limit-download"}
        skip_keys = {"--json"}
        while i < len(tokens):
            t = tokens[i].strip('"')
            if t == "--no-scan":
                no_scan = True
            elif t == "--use-fs-snapshot":
                use_vss = True
            elif t in skip_keys:
                pass
            elif t == "--exclude" or t == "-e":
                if i + 1 < len(tokens):
                    excludes.append(tokens[i + 1].strip('"'))
                    i += 1
            elif t.startswith("--exclude="):
                excludes.append(t.split("=", 1)[1].strip('"'))
            elif t.startswith("-"):
                if t in known_with_arg and i + 1 < len(tokens):
                    extra.append(t)
                    extra.append(tokens[i + 1].strip('"'))
                    i += 1
                else:
                    extra.append(t)
            i += 1
        return {"no_scan": no_scan, "use_vss": use_vss, "exclude": excludes, "extra_args": extra}

    def _sched_load(self):
        if not self.pid:
            return
        entries = self._sched_refresh_manager()
        if entries:
            self._sched_load_form_from_entry(entries[0])
        else:
            # новая задача — все галки включены (кроме --no-scan, он в overlay)
            self.chk_sched_forget.setChecked(True)
            self.chk_sched_tg.setChecked(True)
            self._sched_reset_overlay()
            self._sched_toggle_ui()

    def _sched_apply(self, update_existing: bool = False):
        if not self.pid:
            QMessageBox.warning(self, "Планировщик", "Выберите профиль"); return
        self._autosave()
        prof = self.cfg.profile(self.pid) or {}
        entries = self._sched_profile_entries(prof)
        sched = self._sched_normalize_entry(self._sched_collect())
        signature = self._sched_entry_signature(sched)
        if update_existing:
            task_id, legacy = self._sched_selected_key()
            current = self._sched_find_entry_by_key(entries, task_id, legacy)
            if not current:
                QMessageBox.warning(self, "Планировщик", "Выберите существующую задачу в менеджере ниже.")
                return
            for entry in entries:
                if entry is current:
                    continue
                if self._sched_entry_signature(entry) == signature:
                    QMessageBox.information(self, "Планировщик", "Такая задача уже существует.")
                    self._sched_refresh_manager(select_key=(str(entry.get("task_id") or ""), bool(entry.get("_legacy"))))
                    return
            updated = self._sched_normalize_entry(sched, task_id=current.get("task_id", ""), legacy=False)
            idx = entries.index(current)
            entries[idx] = updated
            ok, msg = SchedulerManager.apply(self.cfg, self.pid, updated)
            if ok:
                self._sched_store_entries(prof, entries)
                self.cfg.save_profile(self.pid, prof)
                self.statusBar().showMessage("✅ Задача обновлена", 5000)
                self._sched_refresh_status(select_key=(updated.get("task_id", ""), False))
            else:
                QMessageBox.critical(self, "Планировщик", f"Не удалось обновить задачу:\n{msg}")
            return
        for entry in entries:
            if self._sched_entry_signature(entry) == signature:
                QMessageBox.information(self, "Планировщик", "Такая задача уже есть. Выберите её в менеджере.")
                self._sched_refresh_status(select_key=(str(entry.get("task_id") or ""), False))
                return
        if entries:
            # Модель хранения — одна задача на профиль (prof["sched"]); вторая
            # задача раньше молча терялась в _sched_store_entries (entries[0]).
            QMessageBox.information(
                self, "Планировщик",
                "У профиля может быть только одна задача расписания.\n"
                "Обновите существующую задачу или добавьте в неё ещё одно время.")
            return
        new_entry = self._sched_normalize_entry(sched, task_id=uuid.uuid4().hex[:8], legacy=False)
        ok, msg = SchedulerManager.apply(self.cfg, self.pid, new_entry)
        if ok:
            entries.append(new_entry)
            self._sched_store_entries(prof, entries)
            self.cfg.save_profile(self.pid, prof)
            self.statusBar().showMessage("✅ Создана новая задача", 5000)
            self._sched_refresh_status(select_key=(new_entry.get("task_id", ""), False))
        else:
            QMessageBox.critical(self, "Планировщик", f"Не удалось создать задачу:\n{msg}")

    def _sched_delete(self):
        if not self.pid:
            return
        prof = self.cfg.profile(self.pid) or {}
        entries = self._sched_profile_entries(prof)
        task_id, legacy = self._sched_selected_key()
        entry = self._sched_find_entry_by_key(entries, task_id, legacy)
        if not entry:
            QMessageBox.warning(self, "Планировщик", "Выберите задачу для удаления.")
            return
        if QMessageBox.question(self, "Планировщик", "Удалить выбранную задачу?") != QMessageBox.Yes:
            return
        ok, msg = SchedulerManager.delete_task(self.pid, entry)
        if ok:
            entries = [row for row in entries if str(row.get("task_id") or "") != task_id]
            self._sched_store_entries(prof, entries)
            self.cfg.save_profile(self.pid, prof)
            self.statusBar().showMessage("🗑 Задача удалена", 4000)
        else:
            QMessageBox.warning(self, "Планировщик", f"Ошибка:\n{msg}")
        self._sched_refresh_status()

    def _sched_run_now(self):
        if not self.pid:
            return
        # Phase 1: agent handles execution — schedule entry not required, only PID needed
        ok, msg = SchedulerManager.run_now(self.pid, "")
        if ok:
            self.statusBar().showMessage("▶ Задача запущена", 4000)
        else:
            QMessageBox.warning(self, "Планировщик", f"Ошибка:\n{msg}")
        self._sched_refresh_status()

    def _sched_auto_refresh(self):
        """Авто-опрос статуса планировщика каждые 3с — нон-блокирующий."""
        if not self.pid:
            return
        SchedulerManager._refresh_status_cache()
        self._sched_refresh_status()

    def _sched_refresh_status(self, select_key=None):
        self._plist_refresh_sched()  # время запуска справа от имени профиля
        # Обновляем индикатор offline (нон-блокирующий: берём из кэша)
        if hasattr(self, "lbl_agent_offline"):
            self.lbl_agent_offline.setVisible(not SchedulerManager.agent_online())
        if not self.pid:
            self.lbl_sched_status.setText("Выберите профиль")
            self.lbl_sched_status.setStyleSheet("color:gray;font-style:italic;")
            self._sched_refresh_manager(select_key=select_key)
            return
        prof = self.cfg.profile(self.pid) or {}
        entries = self._sched_refresh_manager(select_key=select_key)
        if not entries:
            self.lbl_sched_status.setText("⚫ Задачи не созданы")
            self.lbl_sched_status.setStyleSheet("color:gray;")
            return
        task_id, legacy = self._sched_selected_key()
        selected = self._sched_find_entry_by_key(entries, task_id, legacy) or entries[0]
        st = SchedulerManager.status_task(self.pid, selected)
        status_text = st.get("status", "Нет данных") if st.get("exists") else "Нет данных"
        next_text = st.get("next", "—") if st.get("exists") else "—"
        label = st.get("task_name") or "Планировщик"
        self.lbl_sched_status.setText(
            f"✅ Задач: {len(entries)}\nВыбрана: {label}\nСледующий запуск: {next_text}\nСтатус: {status_text}"
        )
        self.lbl_sched_status.setStyleSheet(("color:green;font-weight:bold;" if st.get("exists") else "color:#885500;font-weight:bold;"))
        # NB: НЕ дёргаем _sched_on_selection_changed() — это перерисовывало форму
        # каждые 3с и стирало правки в «Итоговой команде».

    def _sched_open_taskschd(self):
        try:
            subprocess.Popen(["mmc.exe", "taskschd.msc", "/s"],
                             **hidden_console_subprocess_kwargs())
        except Exception as e:
            QMessageBox.warning(self, "Планировщик", f"Не удалось открыть планировщик:\n{e}")

    def _create_shortcut_now(self):
        created = _create_desktop_shortcut("RestLex")
        if created:
            QMessageBox.information(self, "Ярлык", "✅ Ярлык RestLex создан на рабочем столе.")
        else:
            QMessageBox.information(self, "Ярлык", "Ярлык RestLex уже существует на рабочем столе.")


def _create_desktop_shortcut(name: str = "RestLex") -> bool:
    """Create desktop shortcut. Returns True if created, False if already exists."""
    desktop = Path.home() / "Desktop"
    if not desktop.exists():
        desktop = Path.home() / "Рабочий стол"
    lnk = desktop / f"{name}.lnk"
    if lnk.exists():
        return False
    exe_path = Path(sys.executable).resolve() if getattr(sys, "frozen", False) else Path(__file__).resolve()
    ps = (
        f"$ws=New-Object -ComObject WScript.Shell;"
        f"$s=$ws.CreateShortcut('{lnk}');"
        f"$s.TargetPath='{exe_path}';"
        f"$s.WorkingDirectory='{exe_path.parent}';"
        f"$s.IconLocation='{exe_path},0';"
        f"$s.Save()"
    )
    subprocess.run(["powershell", "-Command", ps], capture_output=True,
                   **hidden_console_subprocess_kwargs())
    return lnk.exists()


# ═══════════════════════════════════════════════════════════════════════════
# main
# ═══════════════════════════════════════════════════════════════════════════
def _cli_install(args):
    import argparse
    parser = argparse.ArgumentParser(description="Restlex CLI установка")
    parser.add_argument("--desktop-shortcut", action="store_true", help="Создать ярлык на рабочем столе")
    parser.add_argument("--user", default="", help="Пользователь для ярлыка (пусто = текущий)")
    parser.add_argument("--install-agent", action="store_true", help="Установить службу RestlexAgent")
    parser.add_argument("--all", action="store_true", help="Полная установка: агент, служба, ярлык")
    p = parser.parse_args(args)
    base = PROGRAMDATA_ROOT / APP_DATA_DIRNAME
    base.mkdir(parents=True, exist_ok=True)
    exe_src = Path(sys.executable) if getattr(sys, "frozen", False) else Path(__file__).resolve()
    exe_dst = base / "restlex.exe"
    if exe_src != exe_dst:
        shutil.copy2(str(exe_src), str(exe_dst))
    print(f"Установлено в: {base}")
    # launcher.version иначе создаётся только при первом запуске GUI, а на сервере
    # его не открывают — и хост всю жизнь показывает версию "?" в веб-отчётах.
    try:
        (base / "launcher.version").write_text(APP_VERSION, encoding="utf-8")
    except Exception as e:
        print(f"Не удалось записать launcher.version: {e}")
    # Copy agent binary
    agent_src = local_agent_path()
    agent_dst = base / "restlex-agent.exe"
    if agent_src.exists() and agent_src != agent_dst:
        shutil.copy2(str(agent_src), str(agent_dst))
        print(f"Агент скопирован: {agent_dst}")
    do_agent = p.install_agent or p.all
    # Ярлык создаётся только если явно указан --user (или --desktop-shortcut с --user).
    # При install --all без --user ярлык не создаётся ни для кого.
    do_shortcut = bool((p.user or "").strip())
    # Install service
    if do_agent:
        if agent_dst.exists():
            try:
                kw = {"capture_output": True, "text": True, "timeout": 30}
                kw.update(hidden_console_subprocess_kwargs())
                r = subprocess.run([str(agent_dst), "install"], **kw)
                if r.returncode == 0:
                    print("Служба RestlexAgent установлена")
                    subprocess.run([str(agent_dst), "start"], **kw)
                    print("Служба RestlexAgent запущена")
                else:
                    print(f"Ошибка установки службы: {r.stderr or r.stdout}")
            except Exception as e:
                print(f"Не удалось установить службу: {e}")
        else:
            print("Агент не найден — служба не установлена")
    if do_shortcut:
        # Определить целевого пользователя
        target_user = (p.user or "").strip()
        if not target_user:
            # Активный интерактивный юзер (а не Administrator под которым запущен install)
            try:
                r = subprocess.run(
                    ["powershell", "-NoProfile", "-Command",
                     "(Get-CimInstance Win32_ComputerSystem).UserName"],
                    capture_output=True, text=True, timeout=10,
                    **hidden_console_subprocess_kwargs())
                full = (r.stdout or "").strip()
                # формат "DOMAIN\\user" → берём после слэша
                target_user = full.split("\\")[-1] if full else os.environ.get("USERNAME", "")
            except Exception:
                target_user = os.environ.get("USERNAME", "")

        # Профили пользователей в C:\Users\{user}
        users_root = Path(os.environ.get("SystemDrive", "C:")) / "Users"
        targets = []
        if target_user:
            targets.append(users_root / target_user)
        # Public — для всех
        targets.append(users_root / "Public")

        created_any = False
        for home in targets:
            desktop = home / "Desktop"
            if not desktop.exists():
                desktop = home / "Рабочий стол"
            if not desktop.exists():
                continue
            lnk = desktop / "Restlex.lnk"
            if lnk.exists():
                print(f"Ярлык уже существует: {lnk}")
                created_any = True
                break
            try:
                from win32com.client import Dispatch
                shell = Dispatch("WScript.Shell")
                shortcut = shell.CreateShortCut(str(lnk))
                shortcut.Targetpath = str(exe_dst)
                shortcut.WorkingDirectory = str(base)
                shortcut.IconLocation = str(exe_dst)
                shortcut.save()
                print(f"Ярлык создан: {lnk}  (пользователь: {home.name})")
                created_any = True
                break
            except Exception as e:
                print(f"Не удалось создать ярлык в {desktop}: {e}")
        if not created_any:
            print("Не удалось создать ярлык ни для одного пользователя")
    return 0

def _cli_export_config(args):
    import argparse
    parser = argparse.ArgumentParser(description="Restlex CLI экспорт конфига")
    parser.add_argument("path", help="Путь для сохранения JSON")
    parser.add_argument("--no-secrets", action="store_true",
                        help="Не выгружать пароли/токены (для эталона на общей шаре)")
    p = parser.parse_args(args)
    try:
        cfg = Config()

        # Start with stored values (already decrypted by Config._ld)
        global_data = dict(cfg._g)
        # Force-include keys that have defaults and might not be explicitly saved yet
        _always_export = {
            "update_server_url":         cfg.update_server_url,
            "update_api_key":            cfg.update_api_key,
            "auto_update":               cfg.auto_update,
            "report_url":                cfg.report_url,
            "report_api_key":            cfg.report_api_key,
            "global_pre_backup_cmd":     cfg.global_pre_backup_cmd,
            "global_backup_timeout_hours": cfg.global_backup_timeout_hours,
            "global_no_scan":            cfg.global_no_scan,
            "global_post_backup_check":  cfg.global_post_backup_check,
            "global_check_host_only":    cfg.global_check_host_only,
            "global_vss_timeout_s":      cfg.global_vss_timeout_s,
            "cache_cleanup":             cfg.cache_cleanup,
            "tg_token":                  cfg.tg_token,
            "tg_chat":                   cfg.tg_chat,
            "tg_group":                  cfg.tg_group,
            "tg_fatal":                  cfg.tg_fatal,
            "tg_file":                   cfg.tg_file,
            "tg_prune":                  cfg.tg_prune,
        }
        global_data.update(_always_export)

        data = {
            "global":    global_data,
            "profiles":  dict(cfg._pr),
            "webserver": WebServerManager.load_config(),
        }
        if p.no_secrets:
            data = _strip_export_secrets(data)
        Path(p.path).write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding="utf-8")
        if p.no_secrets:
            print(f"Конфиг экспортирован БЕЗ секретов: {p.path}")
            print("Пароли задать на целевой машине: restlex.exe set repo_password <pw> --profile <pid>")
        else:
            print(f"Конфиг экспортирован: {p.path}")
            print("ВНИМАНИЕ: файл содержит пароли и токены в открытом виде.")
        return 0
    except Exception as e:
        print(f"Ошибка экспорта: {e}")
        return 1

def _cli_import_config(args):
    import argparse
    parser = argparse.ArgumentParser(description="Restlex CLI импорт конфига")
    parser.add_argument("path", help="Путь к JSON-файлу конфига")
    p = parser.parse_args(args)
    path = Path(p.path)
    if not path.exists():
        print(f"Файл не найден: {path}")
        return 1
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        cfg = Config()
        # См. _import_all: экспорт "без секретов" кладёт "" вместо tg_token/
        # repo_password/etc., не убирая ключ — импорт этих пустых значений не
        # должен стирать уже настроенные секреты на машине-адресате.
        if "global" in data:
            for k, v in data["global"].items():
                if k in _EXPORT_SECRET_GLOBAL_KEYS and not v:
                    continue
                cfg.sg(k, v)
        if "profiles" in data:
            for pid, prof in data["profiles"].items():
                if isinstance(prof, dict):
                    existing = cfg.profile(pid) or {}
                    for k in _EXPORT_SECRET_PROFILE_KEYS:
                        if not prof.get(k) and existing.get(k):
                            prof[k] = existing[k]
                cfg.save_profile(pid, prof)
        if "webserver" in data:
            wscfg = data["webserver"]
            # secret_key is machine-specific — regenerate on target
            wscfg.pop("secret_key", None)
            WebServerManager.save_config(wscfg)
            print("Конфиг веб-сервера восстановлен")
        # Re-register Windows Task Scheduler tasks from sched_tasks in profiles
        _tasks_rereg = 0
        for pid, prof in cfg._pr.items():
            for task in (prof.get("sched_tasks") or []):
                if not isinstance(task, dict) or not task.get("task_id"):
                    continue
                try:
                    SchedulerManager.apply_task(cfg, pid, task)
                    _tasks_rereg += 1
                except Exception:
                    pass
        if _tasks_rereg:
            print(f"Зарегистрировано задач планировщика: {_tasks_rereg}")
        print("Конфиг импортирован.")
        _cli_agent_reload()
        return 0
    except Exception as e:
        print(f"Ошибка импорта: {e}")
        return 1

def _cli_agent_reload():
    """Просит службу перечитать конфиг. Служба может быть не установлена — это не ошибка."""
    try:
        agent_api_request("/reload", method="POST", timeout=5)
        print("Служба RestlexAgent перечитала конфиг.")
        return True
    except Exception as e:
        print(f"Служба не откликнулась на /reload ({e}) — конфиг применится при её старте.")
        return False


def _cli_profiles(args):
    """Список профилей: pid и имя. Нужен, чтобы узнать pid для `set --profile`."""
    cfg = Config()
    if not cfg._pr:
        print("Профилей нет.")
        return 0
    for pid, prof in cfg._pr.items():
        name = (prof or {}).get("name", "") if isinstance(prof, dict) else ""
        print(f"{pid}\t{name}")
    return 0


def _cli_set(args):
    import argparse
    parser = argparse.ArgumentParser(description="Restlex CLI правка одного параметра")
    parser.add_argument("key", help="Имя параметра (например repo_password, tg_token)")
    parser.add_argument("value", help="Значение. true/false/число распознаются автоматически")
    parser.add_argument("--profile", default="", help="ID профиля (без него — глобальный параметр)")
    p = parser.parse_args(args)

    # Только true/false -> bool. Числа НЕ преобразуем: пароль "12345" в виде int
    # проходит мимо DPAPI-шифрования (_sv шифрует лишь str) и роняет запуск restic,
    # где он уходит в env RESTIC_PASSWORD. Цена ошибки выше удобства ввода.
    if p.value in ("true", "false"):
        value = (p.value == "true")
    else:
        value = p.value

    secret = p.key in _EXPORT_SECRET_PROFILE_KEYS or p.key in _EXPORT_SECRET_GLOBAL_KEYS
    shown = "***" if secret else value

    cfg = Config()
    if p.profile:
        prof = cfg._pr.get(p.profile)
        if not isinstance(prof, dict):
            print(f"Профиль не найден: {p.profile}. Список: restlex.exe profiles")
            return 1
        prof = dict(prof)
        prof[p.key] = value
        cfg.save_profile(p.profile, prof)
        print(f"Профиль {p.profile}: {p.key} = {shown}")
    else:
        cfg.sg(p.key, value)
        print(f"Глобально: {p.key} = {shown}")
    _cli_agent_reload()
    return 0


def _cli_help():
    print(r"""RestLex v1.0.0 — GUI-клиент для restic

Использование:
  restlex.exe [команда] [опции]

Команды:
  (без команд)                    Запустить GUI
  --help, -h, /?                  Показать эту справку
  --version                       Показать версию
  --run-backup <profile_id>       Автозапуск бэкапа указанного профиля
  --install-shortcut [name]       Создать ярлык RestLex на рабочем столе (если нет), затем запустить GUI
  --no-scan                       Принудительно отключить сканирование (с --run-backup)
  --check                         Принудительно включить check после бэкапа (с --run-backup)
  --pre-cmd "<cmd>"               Выполнить команду перед бэкапом (с --run-backup)
  install                         Установить в ProgramData\RestLex\
    --all                         Полная установка: агент, служба, ярлык
    --desktop-shortcut            Создать ярлык на рабочем столе
    --install-agent               Установить службу RestlexAgent
  export-config <path.json>       Экспортировать весь конфиг
    --no-secrets                  Без паролей/токенов (эталон для общей шары)
  import-config <path.json>       Импортировать конфиг (+ reload службы)
  profiles                        Список профилей (id и имя)
  set <key> <value>               Изменить один параметр (+ reload службы)
    --profile <id>                Параметр профиля, а не глобальный
  reload                          Попросить службу перечитать конфиг

Развёртывание на сервере без GUI:
  restlex.exe install --all
  restlex.exe import-config \\share\etalon.json
  restlex.exe profiles
  restlex.exe set repo_password "<pw>" --profile <id>
""")
    return 0

def _install_crash_logger():
    """Логирует необработанные исключения в %ProgramData%\\RestLex\\logs\\gui_crash.log.
    Без этого падение GUI исчезает без следа (особенно в собранном exe)."""
    import traceback as _tb
    try:
        log_dir = Path(os.environ.get("ProgramData", r"C:\ProgramData")) / "RestLex" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        crash_log = log_dir / "gui_crash.log"
    except Exception:
        crash_log = None

    def _hook(exc_type, exc_val, exc_tb):
        if crash_log is not None:
            try:
                with open(crash_log, "a", encoding="utf-8") as f:
                    f.write(f"\n===== CRASH {datetime.now().isoformat()} (RestLex {APP_VERSION}) =====\n")
                    _tb.print_exception(exc_type, exc_val, exc_tb, file=f)
            except Exception:
                pass
        sys.__excepthook__(exc_type, exc_val, exc_tb)

    sys.excepthook = _hook

def _default_language(cfg, system_lang: str) -> str:
    """Существующая установка (профили или настройки организации) остаётся на
    русском: английская локаль сервера не должна внезапно переключить GUI и
    Telegram-уведомления агента у текущих клиентов. Локаль — только для новых."""
    existing = bool(cfg._pr) or any(cfg.gg(k) for k in (
        "report_url", "tg_token", "tg_chat", "restic", "update_api_key", "agent_auto_update_url"))
    return "ru" if existing else system_lang


def _init_language(app) -> str:
    """Interface language: config.json \"language\"; on first launch — from the Windows locale
    (saved so that the agent and later launches see the same language)."""
    import i18n
    cfg = Config()
    lang = cfg.gg("language", "")
    if lang not in ("ru", "en"):
        lang = _default_language(cfg, i18n.system_language())
        cfg.sg("language", lang)
    i18n.install(app, lang)
    return lang


def main():
    _install_crash_logger()
    raw = sys.argv[1:]
    if raw and raw[0] in ("--help", "-h", "/?"):
        sys.exit(_cli_help())
    if raw and raw[0] == "--version":
        print(f"RestLex {APP_VERSION}")
        sys.exit(0)
    if raw and raw[0] == "--uninstall-agent":
        try:
            run_agent_cmd(["stop"])
        except Exception:
            pass
        try:
            run_agent_cmd(["uninstall"])
            print("RestlexAgent удалён.")
        except Exception as e:
            print(f"Ошибка: {e}")
        sys.exit(0)
    if raw and raw[0] == "install":
        sys.exit(_cli_install(raw[1:]))
    if raw and raw[0] == "export-config":
        sys.exit(_cli_export_config(raw[1:]))
    if raw and raw[0] == "import-config":
        sys.exit(_cli_import_config(raw[1:]))
    if raw and raw[0] == "set":
        sys.exit(_cli_set(raw[1:]))
    if raw and raw[0] == "profiles":
        sys.exit(_cli_profiles(raw[1:]))
    if raw and raw[0] == "reload":
        sys.exit(0 if _cli_agent_reload() else 1)
    # --install-shortcut: create desktop shortcut silently, then launch GUI normally
    if raw and raw[0] == "--install-shortcut":
        _create_desktop_shortcut("RestLex")
        raw = raw[2:] if len(raw) > 1 else []

    # CLI backup flags
    run_backup_pid = None
    cli_no_scan = False
    cli_check = False
    cli_pre_cmd = ""
    i = 0
    while i < len(raw):
        arg = raw[i]
        if arg == "--run-backup" and i + 1 < len(raw):
            run_backup_pid = raw[i + 1]
            i += 2
        elif arg == "--no-scan":
            cli_no_scan = True
            i += 1
        elif arg == "--check":
            cli_check = True
            i += 1
        elif arg == "--pre-cmd" and i + 1 < len(raw):
            cli_pre_cmd = raw[i + 1]
            i += 2
        else:
            i += 1

    set_windows_app_id()
    app=QApplication(sys.argv)
    app.setFont(QFont("Segoe UI",9))
    app.setStyleSheet(STYLE)
    _init_language(app)
    ic = app_icon()
    if not ic.isNull():
        app.setWindowIcon(ic)
    # CLI-флаги передаём В КОНСТРУКТОР: _init_ui() читает _cli_run_backup_pid уже
    # внутри __init__, а присваивание после конструктора приходило слишком поздно —
    # `restlex.exe --run-backup <pid>` просто открывал GUI и бэкап не запускался.
    w=RestlexUI(cli_run_backup_pid=run_backup_pid, cli_no_scan=cli_no_scan,
                cli_check=cli_check, cli_pre_cmd=cli_pre_cmd)
    w.show()
    sys.exit(app.exec_())

if __name__=="__main__":
    main()

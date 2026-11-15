#!/usr/bin/env python3
"""
RestLex Installer
Загружает restlex.exe, restlex-agent.exe и конфиг с update-сервера и устанавливает их.

CLI:
  restlex-installer.exe                          GUI wizard
  restlex-installer.exe install --all            Тихая установка (без ярлыка)
  restlex-installer.exe install --all --user <n> Тихая установка + ярлык юзеру
  restlex-installer.exe install --all --shortcut-all  Установка + ярлык всем
  restlex-installer.exe update                   Тихое обновление (если новее)
  restlex-installer.exe uninstall                Тихое удаление
  restlex-installer.exe status                   Версии (установлена / на сервере)
"""

import sys, os, json, shutil, subprocess, argparse
from pathlib import Path
from typing import Optional, Callable

import requests
from PyQt5.QtWidgets import (
    QApplication, QWizard, QWizardPage, QLabel, QVBoxLayout, QHBoxLayout,
    QRadioButton, QButtonGroup, QProgressBar, QTextEdit, QCheckBox,
    QSizePolicy, QGroupBox
)
from PyQt5.QtCore import Qt, QThread, pyqtSignal
from PyQt5.QtGui import QFont, QIcon, QPixmap

# ── Вшитые константы (компилируются в exe) ─────────────────────────────────────
_SERVER  = "http://1221.by:3266"
_API_KEY = os.environ.get("RESTLEX_UPDATE_KEY", "")  # задаётся при сборке/запуске, не в исходниках
_HDRS    = {"X-Api-Key": _API_KEY}

# ОТДЕЛЬНЫЙ токен для restlex-config.json (заголовок X-Install-Key).
# Ключ обновлений (_API_KEY) зашит в каждый агент парка — раздавать по нему
# плейнтекстовый конфиг с repo_password/tg_token нельзя. На сервере тот же
# токен задаётся полем `install_key` в update-server.json (или --install-key
# при install); не задан — сервер отдаёт 404, установка продолжается без
# серверного конфига.
# ОГРАНИЧЕНИЕ: это ОБФУСКАЦИЯ, а не защита — токен всё равно вшит в
# инсталлятор и едет по HTTP без TLS. XOR лишь повторяет поведение
# Rust-инсталляторов и убирает голую строку из бинарника; он отвязывает
# выдачу конфига от ключа обновлений и делает её ротируемой, но не заменяет TLS.
def _install_key() -> str:
    return os.environ.get("RESTLEX_INSTALL_KEY", "")

DATA_DIR    = Path(os.environ.get("ProgramData", r"C:\ProgramData")) / "RestLex"
STATE_FILE  = DATA_DIR / "installer.json"
RESTLEX_EXE = DATA_DIR / "restlex.exe"
AGENT_EXE   = DATA_DIR / "restlex-agent.exe"
CONFIG_JSON = DATA_DIR / "config.json"


# ═══════════════════════════════════════════════════════════════════════════════
# Утилиты версий
# ═══════════════════════════════════════════════════════════════════════════════

def ver_tuple(v: str) -> tuple:
    try:
        return tuple(int(x) for x in str(v).split("."))
    except Exception:
        return (0, 0, 0)


def get_state() -> dict:
    if STATE_FILE.exists():
        try:
            return json.loads(STATE_FILE.read_text(encoding="utf-8"))
        except Exception:
            pass
    return {}


def save_state(gui_ver: str, agent_ver: str):
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    STATE_FILE.write_text(
        json.dumps({"gui_version": gui_ver, "agent_version": agent_ver}, indent=2),
        encoding="utf-8"
    )
    (DATA_DIR / "launcher.version").write_text(gui_ver, encoding="utf-8")
    (DATA_DIR / "agent.version").write_text(agent_ver, encoding="utf-8")


# ═══════════════════════════════════════════════════════════════════════════════
# Работа с сервером обновлений
# ═══════════════════════════════════════════════════════════════════════════════

def fetch_manifest() -> Optional[dict]:
    try:
        r = requests.get(f"{_SERVER}/api/manifest", headers=_HDRS, timeout=20)
        r.raise_for_status()
        return r.json()
    except Exception:
        return None


def download_file(name: str, dest: Path, on_progress: Optional[Callable] = None,
                  extra_headers: Optional[dict] = None) -> bool:
    tmp = dest.with_suffix(".tmp")
    try:
        headers = dict(_HDRS)
        if extra_headers:
            headers.update(extra_headers)
        r = requests.get(f"{_SERVER}/api/download", headers=headers,
                         params={"file": name}, stream=True, timeout=300)
        r.raise_for_status()
        total = int(r.headers.get("content-length", 0))
        done = 0
        dest.parent.mkdir(parents=True, exist_ok=True)
        with open(tmp, "wb") as f:
            for chunk in r.iter_content(65536):
                f.write(chunk)
                done += len(chunk)
                if on_progress and total:
                    on_progress(int(done * 100 / total))
        tmp.replace(dest)
        return True
    except Exception:
        try:
            tmp.unlink(missing_ok=True)
        except Exception:
            pass
        return False


def _run(cmd: list) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, creationflags=0x08000000)


def _create_public_shortcut():
    pub_desktop = Path(os.environ.get("PUBLIC", r"C:\Users\Public")) / "Desktop"
    lnk = pub_desktop / "Restlex.lnk"
    if lnk.exists():
        return
    ps = (
        f"$s=(New-Object -COM WScript.Shell).CreateShortcut('{lnk}');"
        f"$s.TargetPath='{RESTLEX_EXE}';"
        f"$s.WorkingDirectory='{DATA_DIR}';"
        f"$s.IconLocation='{RESTLEX_EXE},0';"
        f"$s.Save()"
    )
    subprocess.run(["powershell", "-NoProfile", "-Command", ps],
                   capture_output=True, creationflags=0x08000000)


# ═══════════════════════════════════════════════════════════════════════════════
# Логика установки / обновления / удаления
# ═══════════════════════════════════════════════════════════════════════════════

def do_install(shortcut: str = "none", log=print, on_progress=None) -> bool:
    """
    shortcut: "user" | "all" | "none"
    """
    log("Подключение к серверу обновлений...")
    manifest = fetch_manifest()
    if not manifest:
        log("ОШИБКА: Нет связи с сервером обновлений.")
        return False

    gui_ver   = manifest.get("gui_version", "0.0.0")
    agent_ver = manifest.get("agent_version", "0.0.0")
    log(f"Доступно: GUI {gui_ver}, агент {agent_ver}")
    DATA_DIR.mkdir(parents=True, exist_ok=True)

    def progress(tag, pct):
        if on_progress:
            on_progress(tag, pct)

    log("Загрузка restlex.exe...")
    if not download_file("restlex.exe", RESTLEX_EXE,
                         lambda p: progress("restlex.exe", p)):
        log("ОШИБКА: не удалось загрузить restlex.exe")
        return False

    log("Загрузка restlex-agent.exe...")
    if not download_file("restlex-agent.exe", AGENT_EXE,
                         lambda p: progress("restlex-agent.exe", p)):
        log("ОШИБКА: не удалось загрузить restlex-agent.exe")
        return False

    if not CONFIG_JSON.exists():
        log("Загрузка конфигурации...")
        # restlex-config.json раздаётся только по X-Install-Key. Отсутствие
        # конфига (404/401) — ПРЕДУПРЕЖДЕНИЕ, а не ошибка: установка идёт дальше.
        if not download_file("restlex-config.json", CONFIG_JSON,
                             lambda p: progress("config", p),
                             extra_headers={"X-Install-Key": _install_key()}):
            log("ПРЕДУПРЕЖДЕНИЕ: конфиг парка не загружен "
                "(сервер не раздаёт restlex-config.json). Установка продолжается.")

    log("Установка службы RestlexAgent...")
    if shortcut == "user":
        username = os.environ.get("USERNAME", "")
        r = _run([str(RESTLEX_EXE), "install", "--all", "--user", username])
    else:
        r = _run([str(RESTLEX_EXE), "install", "--all"])

    if r.returncode != 0:
        log(f"ОШИБКА установки службы: {r.stderr or r.stdout}")
        return False

    if shortcut == "all":
        log("Создание ярлыка для всех пользователей...")
        _create_public_shortcut()

    save_state(gui_ver, agent_ver)
    log("Установка завершена!")
    return True


def do_update(log=print, on_progress=None) -> bool:
    log("Проверка обновлений...")
    manifest = fetch_manifest()
    if not manifest:
        log("ОШИБКА: Нет связи с сервером.")
        return False

    gui_ver   = manifest.get("gui_version", "0.0.0")
    agent_ver = manifest.get("agent_version", "0.0.0")
    state     = get_state()
    cur_gui   = state.get("gui_version", "0.0.0")
    cur_agent = state.get("agent_version", "0.0.0")

    if ver_tuple(gui_ver) <= ver_tuple(cur_gui) and ver_tuple(agent_ver) <= ver_tuple(cur_agent):
        log("Установлена актуальная версия. Обновление не требуется.")
        return True

    log(f"GUI {cur_gui} → {gui_ver}, агент {cur_agent} → {agent_ver}")

    def progress(tag, pct):
        if on_progress:
            on_progress(tag, pct)

    _run([str(AGENT_EXE), "stop"])

    log("Загрузка restlex.exe...")
    if not download_file("restlex.exe", RESTLEX_EXE,
                         lambda p: progress("restlex.exe", p)):
        log("ОШИБКА: restlex.exe")
        return False

    log("Загрузка restlex-agent.exe...")
    if not download_file("restlex-agent.exe", AGENT_EXE,
                         lambda p: progress("restlex-agent.exe", p)):
        log("ОШИБКА: restlex-agent.exe")
        return False

    _run([str(AGENT_EXE), "install"])
    _run([str(AGENT_EXE), "start"])
    save_state(gui_ver, agent_ver)
    log("Обновление завершено.")
    return True


def do_uninstall(log=print) -> bool:
    if RESTLEX_EXE.exists():
        log("Удаление службы RestlexAgent...")
        _run([str(RESTLEX_EXE), "--uninstall-agent"])

    log("Удаление файлов...")
    for f in [RESTLEX_EXE, AGENT_EXE, STATE_FILE]:
        try:
            f.unlink(missing_ok=True)
        except Exception:
            pass

    log("Удаление ярлыков...")
    pub = Path(os.environ.get("PUBLIC", r"C:\Users\Public")) / "Desktop" / "Restlex.lnk"
    pub.unlink(missing_ok=True)
    users_root = Path(os.environ.get("SystemDrive", "C:")) / "Users"
    if users_root.exists():
        for user_dir in users_root.iterdir():
            for desktop_name in ["Desktop", "Рабочий стол"]:
                lnk = user_dir / desktop_name / "Restlex.lnk"
                if lnk.exists():
                    lnk.unlink(missing_ok=True)

    log("Удаление завершено.")
    return True


def get_status() -> dict:
    state    = get_state()
    manifest = fetch_manifest()
    return {
        "installed":            RESTLEX_EXE.exists(),
        "gui_version":          state.get("gui_version", "—"),
        "agent_version":        state.get("agent_version", "—"),
        "server_gui_version":   manifest.get("gui_version", "—") if manifest else "недоступен",
        "server_agent_version": manifest.get("agent_version", "—") if manifest else "недоступен",
    }


# ═══════════════════════════════════════════════════════════════════════════════
# Worker-поток для GUI
# ═══════════════════════════════════════════════════════════════════════════════

class InstallWorker(QThread):
    log_sig      = pyqtSignal(str)
    progress_sig = pyqtSignal(str, int)   # (file_tag, 0-100)
    done_sig     = pyqtSignal(bool)       # success

    def __init__(self, action: str, shortcut: str = "none"):
        super().__init__()
        self.action   = action    # "install" | "update" | "uninstall"
        self.shortcut = shortcut  # "user" | "all" | "none"

    def run(self):
        def log(msg):
            self.log_sig.emit(msg)

        def on_progress(tag, pct):
            self.progress_sig.emit(tag, pct)

        if self.action == "install":
            ok = do_install(self.shortcut, log=log, on_progress=on_progress)
        elif self.action == "update":
            ok = do_update(log=log, on_progress=on_progress)
        elif self.action == "uninstall":
            ok = do_uninstall(log=log)
        else:
            ok = False

        self.done_sig.emit(ok)


# ═══════════════════════════════════════════════════════════════════════════════
# GUI Wizard
# ═══════════════════════════════════════════════════════════════════════════════

PAGE_WELCOME  = 0
PAGE_OPTIONS  = 1
PAGE_PROGRESS = 2
PAGE_FINISH   = 3

# Shared wizard state
_wiz_action   = "install"   # "install" | "update" | "uninstall"
_wiz_shortcut = "none"      # "user" | "all" | "none"
_wiz_success  = False


class WelcomePage(QWizardPage):
    def __init__(self, manifest: Optional[dict]):
        super().__init__()
        self.manifest  = manifest
        self.state     = get_state()
        self.installed = RESTLEX_EXE.exists() and bool(self.state)
        self._action_btns = {}
        self._setup_ui()

    def _setup_ui(self):
        global _wiz_action
        self.setTitle("Установщик RestLex")

        layout = QVBoxLayout(self)
        layout.setSpacing(12)

        # ── Версии ──────────────────────────────────────────────────────────────
        srv_gui   = self.manifest.get("gui_version",   "—") if self.manifest else "недоступен"
        srv_agent = self.manifest.get("agent_version", "—") if self.manifest else "недоступен"
        cur_gui   = self.state.get("gui_version",   "—")
        cur_agent = self.state.get("agent_version", "—")

        info_box = QGroupBox("Версии")
        info_lay = QVBoxLayout(info_box)
        info_lay.setSpacing(4)

        if self.installed:
            info_lay.addWidget(QLabel(f"Установлено:  GUI {cur_gui},  агент {cur_agent}"))
        else:
            info_lay.addWidget(QLabel("RestLex не установлен"))

        srv_lbl = QLabel(
            f"На сервере:   GUI {srv_gui},  агент {srv_agent}"
            if self.manifest else "Сервер обновлений недоступен"
        )
        info_lay.addWidget(srv_lbl)
        layout.addWidget(info_box)

        # ── Выбор действия ──────────────────────────────────────────────────────
        action_box = QGroupBox("Действие")
        action_lay = QVBoxLayout(action_box)
        action_lay.setSpacing(6)

        self._bg = QButtonGroup(self)
        options  = self._build_action_options(cur_gui, srv_gui)

        for i, (label, action) in enumerate(options):
            rb = QRadioButton(label)
            if i == 0:
                rb.setChecked(True)
                _wiz_action = action
            self._bg.addButton(rb, i)
            self._action_btns[i] = action
            action_lay.addWidget(rb)

        self._bg.idClicked.connect(self._on_action_changed)
        layout.addWidget(action_box)

        if not self.manifest and not self.installed:
            warn = QLabel("⚠  Нет связи с сервером обновлений. Установка невозможна.")
            warn.setStyleSheet("color: #c0392b; font-weight: bold;")
            layout.addWidget(warn)

        layout.addStretch()

    def _build_action_options(self, cur_gui: str, srv_gui: str) -> list:
        global _wiz_action
        if not self.installed:
            srv = self.manifest.get("gui_version", "?") if self.manifest else "?"
            return [(f"Установить RestLex {srv}", "install")]

        has_server = self.manifest is not None
        options = []

        if has_server:
            srv = self.manifest.get("gui_version", "?")
            cur = cur_gui
            if ver_tuple(srv) > ver_tuple(cur):
                options.append((f"Обновить  {cur} → {srv}", "update"))
            elif ver_tuple(srv) < ver_tuple(cur):
                options.append((f"Откатить  {cur} → {srv}", "update"))
            else:
                options.append((f"Переустановить  {cur}", "install"))
        else:
            options.append(("Переустановить (сервер недоступен — нет загрузки)", "install"))

        options.append(("Удалить RestLex", "uninstall"))
        return options

    def _on_action_changed(self, btn_id: int):
        global _wiz_action
        _wiz_action = self._action_btns.get(btn_id, "install")
        self.completeChanged.emit()

    def isComplete(self) -> bool:
        if not self.manifest and not self.installed:
            return False
        if _wiz_action == "install" and not self.manifest:
            return False
        return True

    def nextId(self) -> int:
        if _wiz_action == "uninstall":
            return PAGE_PROGRESS
        return PAGE_OPTIONS


class OptionsPage(QWizardPage):
    def __init__(self):
        super().__init__()
        self.setTitle("Параметры установки")
        self._setup_ui()

    def _setup_ui(self):
        global _wiz_shortcut
        layout = QVBoxLayout(self)
        layout.setSpacing(12)

        sc_box = QGroupBox("Ярлык на рабочем столе")
        sc_lay = QVBoxLayout(sc_box)
        sc_lay.setSpacing(6)

        self._sc_bg = QButtonGroup(self)
        rb_user = QRadioButton("Текущему пользователю")
        rb_all  = QRadioButton("Всем пользователям (Public Desktop)")
        rb_none = QRadioButton("Не создавать")
        rb_user.setChecked(True)
        _wiz_shortcut = "user"

        for i, rb in enumerate([rb_user, rb_all, rb_none]):
            self._sc_bg.addButton(rb, i)
            sc_lay.addWidget(rb)

        self._sc_bg.idClicked.connect(self._on_sc_changed)
        layout.addWidget(sc_box)
        layout.addStretch()

    def _on_sc_changed(self, btn_id: int):
        global _wiz_shortcut
        _wiz_shortcut = ["user", "all", "none"][btn_id]

    def nextId(self) -> int:
        return PAGE_PROGRESS


class ProgressPage(QWizardPage):
    def __init__(self):
        super().__init__()
        self.setTitle("Выполнение")
        self._done   = False
        self._worker = None
        self._setup_ui()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setSpacing(8)

        self._file_label = QLabel("Подготовка...")
        layout.addWidget(self._file_label)

        self._bar = QProgressBar()
        self._bar.setRange(0, 100)
        self._bar.setValue(0)
        layout.addWidget(self._bar)

        self._log = QTextEdit()
        self._log.setReadOnly(True)
        self._log.setFont(QFont("Consolas", 9))
        self._log.setMinimumHeight(180)
        layout.addWidget(self._log)

    def initializePage(self):
        self._done   = False
        self._log.clear()
        self._bar.setValue(0)
        self._file_label.setText("Запуск...")
        self.wizard().button(QWizard.NextButton).setEnabled(False)
        self.wizard().button(QWizard.BackButton).setEnabled(False)

        self._worker = InstallWorker(_wiz_action, _wiz_shortcut)
        self._worker.log_sig.connect(self._on_log)
        self._worker.progress_sig.connect(self._on_progress)
        self._worker.done_sig.connect(self._on_done)
        self._worker.start()

    def _on_log(self, msg: str):
        self._log.append(msg)

    def _on_progress(self, tag: str, pct: int):
        self._file_label.setText(f"Загрузка: {tag}  {pct}%")
        self._bar.setValue(pct)

    def _on_done(self, success: bool):
        global _wiz_success
        _wiz_success = success
        self._done   = True
        self._bar.setValue(100 if success else self._bar.value())
        self.wizard().button(QWizard.NextButton).setEnabled(True)
        self.completeChanged.emit()

    def isComplete(self) -> bool:
        return self._done

    def nextId(self) -> int:
        return PAGE_FINISH


class FinishPage(QWizardPage):
    def __init__(self):
        super().__init__()
        self.setTitle("Готово")
        self._setup_ui()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setSpacing(12)
        self._msg = QLabel()
        self._msg.setWordWrap(True)
        self._msg.setFont(QFont("Segoe UI", 11))
        layout.addWidget(self._msg)
        self._launch_cb = QCheckBox("Запустить RestLex после закрытия")
        self._launch_cb.setChecked(True)
        layout.addWidget(self._launch_cb)
        layout.addStretch()

    def initializePage(self):
        action_labels = {
            "install":   "Установка",
            "update":    "Обновление",
            "uninstall": "Удаление",
        }
        verb = action_labels.get(_wiz_action, "Операция")

        if _wiz_success:
            self._msg.setText(f"✔  {verb} завершена успешно.")
            self._msg.setStyleSheet("color: #27ae60;")
        else:
            self._msg.setText(f"✖  {verb} завершена с ошибками. Проверьте лог на предыдущем шаге.")
            self._msg.setStyleSheet("color: #c0392b;")
            self._launch_cb.setChecked(False)

        # Hide launch checkbox if uninstalled or failed
        show_launch = _wiz_success and _wiz_action != "uninstall" and RESTLEX_EXE.exists()
        self._launch_cb.setVisible(show_launch)

    def validatePage(self) -> bool:
        if self._launch_cb.isVisible() and self._launch_cb.isChecked():
            try:
                subprocess.Popen([str(RESTLEX_EXE)], creationflags=0x00000008)  # DETACHED_PROCESS
            except Exception:
                pass
        return True

    def nextId(self) -> int:
        return -1


def run_gui():
    app = QApplication(sys.argv)
    app.setApplicationName("RestLex Installer")
    app.setWindowIcon(QIcon(str(Path(__file__).parent / "icon.ico")))

    wizard = QWizard()
    wizard.setWindowTitle("Установщик RestLex")
    wizard.setWizardStyle(QWizard.ModernStyle)
    wizard.setMinimumSize(520, 440)

    # Fetch manifest once for welcome page
    manifest = fetch_manifest()

    wizard.setPage(PAGE_WELCOME,  WelcomePage(manifest))
    wizard.setPage(PAGE_OPTIONS,  OptionsPage())
    wizard.setPage(PAGE_PROGRESS, ProgressPage())
    wizard.setPage(PAGE_FINISH,   FinishPage())

    wizard.setStartId(PAGE_WELCOME)
    wizard.show()
    sys.exit(app.exec_())


# ═══════════════════════════════════════════════════════════════════════════════
# CLI режим (тихая установка)
# ═══════════════════════════════════════════════════════════════════════════════

def cli_install(args):
    parser = argparse.ArgumentParser(prog="restlex-installer install")
    parser.add_argument("--all",          action="store_true")
    parser.add_argument("--user",         default="",   help="Ярлык для пользователя")
    parser.add_argument("--shortcut-all", action="store_true", dest="shortcut_all",
                        help="Ярлык для всех пользователей (Public Desktop)")
    p = parser.parse_args(args)

    if p.shortcut_all:
        shortcut = "all"
    elif p.user:
        os.environ["USERNAME"] = p.user
        shortcut = "user"
    else:
        shortcut = "none"

    ok = do_install(shortcut=shortcut)
    sys.exit(0 if ok else 1)


def cli_update():
    ok = do_update()
    sys.exit(0 if ok else 1)


def cli_uninstall():
    ok = do_uninstall()
    sys.exit(0 if ok else 1)


def cli_status():
    st = get_status()
    print(f"Установлено:   {'да' if st['installed'] else 'нет'}")
    print(f"GUI версия:    {st['gui_version']}")
    print(f"Агент версия:  {st['agent_version']}")
    print(f"Сервер GUI:    {st['server_gui_version']}")
    print(f"Сервер агент:  {st['server_agent_version']}")
    sys.exit(0)


# ═══════════════════════════════════════════════════════════════════════════════
# Точка входа
# ═══════════════════════════════════════════════════════════════════════════════

def main():
    args = sys.argv[1:]

    if not args:
        run_gui()
        return

    cmd = args[0].lstrip("-").lower()

    if cmd == "install":
        cli_install(args[1:])
    elif cmd == "update":
        cli_update()
    elif cmd == "uninstall":
        cli_uninstall()
    elif cmd == "status":
        cli_status()
    else:
        print(f"Неизвестная команда: {args[0]}")
        print(__doc__)
        sys.exit(1)


if __name__ == "__main__":
    main()

"""Скриншоты всех вкладок GUI для визуальной проверки и README.

    python tools/shoot_ui.py <out_dir> [ru|en]

GUI поднимается из исходников с ProgramData, подменённым на временную папку
с демо-конфигом: боевой конфиг машины и местный агент не затрагиваются.
Модальные окна, всплывшие при старте, тоже снимаются и закрываются.
"""
import json, os, sys, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(sys.argv[1] if len(sys.argv) > 1 else "shots").resolve()
LANG = sys.argv[2] if len(sys.argv) > 2 else ""

tmp = Path(tempfile.mkdtemp(prefix="restlex-ui-"))
os.environ["ProgramData"] = str(tmp)
data = tmp / "RestLex"
data.mkdir(parents=True)
glob = {"report_url": "http://192.168.1.10:8797/report", "update_server_url": "http://1221.by:3266",
        "tg_chat": "-1001234567890", "auto_update": True}
if LANG:
    glob["language"] = LANG
(data / "config.json").write_text(json.dumps(glob), encoding="utf-8")
demo = {
    "a1b2c3d4": {"name": "Documents → NAS", "sources": [r"C:\Users\Public\Documents"],
                 "repo_type": "local", "repo_path": r"\\nas\backup\docs", "use_vss": True,
                 "retention": {"keep-daily": 7, "keep-weekly": 4},
                 "sched": {"freq": "daily", "time": "02:00", "days": "", "interval": 0}},
    "e5f6a7b8": {"name": "1C database → S3", "sources": [r"D:\1C"], "repo_type": "s3",
                 "repo_path": "s3:s3.amazonaws.com/bucket/1c", "use_vss": True,
                 "retention": {"keep-daily": 14, "keep-monthly": 6},
                 "sched": {"freq": "daily", "time": "23:30", "days": "", "interval": 0}},
}
(data / "profiles.json").write_text(json.dumps(demo, ensure_ascii=False), encoding="utf-8")

sys.path.insert(0, str(ROOT))
sys.argv = ["restlex"]
import restlex  # noqa: E402
from PyQt5.QtWidgets import QApplication, QTabWidget, QDialog, QMessageBox  # noqa: E402
from PyQt5.QtGui import QFont  # noqa: E402

OUT.mkdir(parents=True, exist_ok=True)
app = QApplication(sys.argv)
app.setFont(QFont("Segoe UI", 9))
app.setStyleSheet(restlex.STYLE)
restlex._init_language(app)
n = [0]


def save(widget, name):
    n[0] += 1
    p = OUT / f"{n[0]:02d}_{name}.png"
    widget.grab().save(str(p))
    print(p.name)


def pump(sec=0.6):
    end = time.time() + sec
    while time.time() < end:
        app.processEvents()
        time.sleep(0.02)


def close_modals():
    for w in app.topLevelWidgets():
        if w.isVisible() and isinstance(w, (QDialog, QMessageBox)):
            save(w, "modal_" + (w.windowTitle() or "dialog").replace(" ", "_")[:30])
            w.reject() if isinstance(w, QDialog) else w.close()


from PyQt5.QtCore import QTimer  # noqa: E402
guard = QTimer(); guard.timeout.connect(close_modals); guard.start(300)

w = restlex.RestlexUI()
w.resize(1200, 760)
w.show()
pump(3)
if w.plist.count():
    w.plist.setCurrentRow(0)
    w._on_prof_click(w.plist.item(0))
    pump(1)


def safe(name):
    return "".join(c if c.isalnum() else "_" for c in name).strip("_")[:30]


for i in range(w.tabs.count()):
    w.tabs.setCurrentIndex(i)
    pump(1)
    page = w.tabs.widget(i)
    save(w, f"tab{i}_{safe(w.tabs.tabText(i))}")
    for inner in page.findChildren(QTabWidget):
        if inner is w.tabs or not inner.isVisible():
            continue
        for j in range(1, inner.count()):
            inner.setCurrentIndex(j)
            pump(0.8)
            save(w, f"tab{i}_{safe(w.tabs.tabText(i))}__{safe(inner.tabText(j))}")
        inner.setCurrentIndex(0)

close_modals()

# Leftover untranslated texts in visible widgets (for en mode).
if LANG == "en":
    import re as _re
    from PyQt5 import QtWidgets as _W
    cyr, left = _re.compile("[А-Яа-яЁё]"), set()
    for i in range(w.tabs.count()):
        w.tabs.setCurrentIndex(i); pump(0.3)
        for x in w.findChildren(_W.QWidget):
            vals = []
            if isinstance(x, (_W.QLabel, _W.QAbstractButton)): vals.append(x.text())
            if isinstance(x, _W.QGroupBox): vals.append(x.title())
            if isinstance(x, _W.QLineEdit): vals.append(x.placeholderText())
            if isinstance(x, _W.QTabWidget): vals += [x.tabText(k) for k in range(x.count())]
            vals.append(x.toolTip())
            left.update(v for v in vals if v and cyr.search(v))
    (OUT / "untranslated.txt").write_text("\n---\n".join(sorted(left)), encoding="utf-8")
    print("UNTRANSLATED", len(left))
w._closing = True
print("OUT", OUT, "PROGRAMDATA", tmp)
os._exit(0)

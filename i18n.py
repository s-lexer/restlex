"""Перевод интерфейса RestLex без переписывания restlex.py.

Тексты в коде остаются русскими; словарь i18n/en.json сопоставляет русской
строке английскую. Строки с подстановками (f-строки) хранятся шаблонами
"Найдено {0} файлов" -> "Found {0} files".

Подключение: install(app, lang) до создания окон. Перевод выполняется
  * при первом показе каждого виджета (событие Polish) — тексты из конструкторов;
  * в перехваченных сеттерах (setText, setToolTip, QMessageBox…) — тексты,
    которые меняются позже.
Нет перевода — остаётся русский текст (ничего не ломается).
"""
import ctypes, json, re, sys
from pathlib import Path

CYR = re.compile("[А-Яа-яЁё]")
LANG = "ru"
EXACT: dict = {}
_PATTERNS: dict = {}   # первые 3 символа литерального префикса -> [(regex, en_template)]
_WILD: list = []       # шаблоны, начинающиеся с подстановки


def system_language() -> str:
    """ru для русско-/белорусско-/украино-/казахоязычного интерфейса Windows, иначе en."""
    try:
        lid = ctypes.windll.kernel32.GetUserDefaultUILanguage() & 0x3FF
        return "ru" if lid in (0x19, 0x23, 0x22, 0x3F) else "en"
    except Exception:
        return "ru"


def _catalog_path() -> Path:
    base = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parent))
    return base / "i18n" / "en.json"


def _compile(tpl: str):
    parts = re.split(r"(\{\d+\})", tpl.replace("{{", "\0").replace("}}", "\1"))
    rx, prefix = "", None
    for p in parts:
        if re.fullmatch(r"\{\d+\}", p):
            prefix = "" if prefix is None else prefix
            rx += f"(?P<g{p[1:-1]}>.*?)"
        else:
            lit = p.replace("\0", "{").replace("\1", "}")
            if prefix is None:
                prefix = lit
            rx += re.escape(lit)
    return re.compile("^" + rx + "$", re.S), (prefix or "")


def load(lang: str):
    global LANG
    LANG = lang
    EXACT.clear(); _PATTERNS.clear(); _WILD.clear()
    if lang != "en":
        return
    try:
        cat = json.loads(_catalog_path().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return
    for ru, en in cat.items():
        if "{" in ru and re.search(r"\{\d+\}", ru):
            rx, prefix = _compile(ru)
            # Short literal part ("{0} в {1}") matches anything — such a template
            # is accepted only when it translates the string fully (see tr).
            weak = len(CYR.findall(re.sub(r"\{\d+\}", "", ru))) < 6
            (_PATTERNS.setdefault(prefix[:3], []) if prefix else _WILD).append((rx, en, weak))
        else:
            EXACT[ru] = en


def _fill(en: str, m) -> str:
    vals = {k[1:]: tr(v) for k, v in m.groupdict().items()}
    return re.sub(r"\{(\d+)\}", lambda x: vals.get(x.group(1), x.group(0)),
                  en.replace("{{", "\0").replace("}}", "\1")).replace("\0", "{").replace("\1", "}")


def tr(s):
    if LANG != "en" or not isinstance(s, str) or not s or not CYR.search(s):
        return s
    r = EXACT.get(s)
    if r is not None:
        return r
    for rx, en, weak in _PATTERNS.get(s[:3], []) + _WILD:
        m = rx.match(s)
        if m:
            out = _fill(en, m)
            if not (weak and CYR.search(out)):
                return out
    if "\n" in s:
        return "\n".join(tr(x) for x in s.split("\n"))
    return s


def tr_exact(s):
    """Только точное совпадение — для элементов списков, где бывают данные пользователя
    (имя профиля «Бэкап в NAS» не должно переводиться шаблоном)."""
    if LANG != "en" or not isinstance(s, str):
        return s
    return EXACT.get(s, s)


def _wrap(cls, name, idx=(0,)):
    orig = getattr(cls, name)

    def f(*a, **kw):
        a = list(a)
        for i in idx:
            if i < len(a):
                a[i] = tr(a[i])
        return orig(*a, **kw)
    setattr(cls, name, f)


def _wrap_static(cls, name, idx):
    orig = getattr(cls, name)

    def f(*a, **kw):
        a = list(a)
        for i in idx:
            if i < len(a):
                a[i] = tr(a[i])
        return orig(*a, **kw)
    setattr(cls, name, staticmethod(f))


def translate_widget(w):
    """Тексты одного виджета (без детей) — вызывается на Polish."""
    from PyQt5 import QtWidgets as W
    try:
        if isinstance(w, (W.QLabel, W.QAbstractButton)) and CYR.search(w.text() or ""):
            w.setText(w.text())
        if CYR.search(w.toolTip() or ""):
            w.setToolTip(w.toolTip())
        if isinstance(w, W.QLineEdit) and CYR.search(w.placeholderText() or ""):
            w.setPlaceholderText(w.placeholderText())
        if isinstance(w, W.QPlainTextEdit) and CYR.search(w.placeholderText() or ""):
            w.setPlaceholderText(w.placeholderText())
        if isinstance(w, W.QGroupBox) and CYR.search(w.title() or ""):
            w.setTitle(w.title())
        if isinstance(w, W.QTabWidget):
            for i in range(w.count()):
                w.setTabText(i, w.tabText(i))
                w.setTabToolTip(i, w.tabToolTip(i))
        if isinstance(w, (W.QSpinBox, W.QDoubleSpinBox)) and CYR.search(w.suffix() or ""):
            w.setSuffix(w.suffix())
        if isinstance(w, W.QListWidget):
            for i in range(w.count()):
                it = w.item(i)
                if CYR.search(it.text() or ""):
                    it.setText(tr_exact(it.text()))
        if isinstance(w, W.QComboBox):
            for i in range(w.count()):
                if CYR.search(w.itemText(i) or ""):
                    w.setItemText(i, tr(w.itemText(i)))
        if isinstance(w, (W.QTreeWidget, W.QTableWidget)):
            hdr = w.headerItem() if isinstance(w, W.QTreeWidget) else None
            if hdr is not None:
                for i in range(hdr.columnCount()):
                    hdr.setText(i, tr(hdr.text(i)))
            if isinstance(w, W.QTableWidget):
                for i in range(w.columnCount()):
                    it = w.horizontalHeaderItem(i)
                    if it is not None:
                        it.setText(tr(it.text()))
        if w.isWindow() and CYR.search(w.windowTitle() or ""):
            w.setWindowTitle(w.windowTitle())
        if isinstance(w, W.QFormLayout):
            pass
    except RuntimeError:
        pass  # виджет уже удалён на стороне C++


def install(app, lang: str):
    load(lang)
    if lang != "en":
        return
    from PyQt5 import QtWidgets as W, QtCore as C

    for cls in (W.QLabel, W.QAbstractButton):
        _wrap(cls, "setText", (1,))
    _wrap(W.QWidget, "setToolTip", (1,))
    _wrap(W.QWidget, "setWindowTitle", (1,))
    _wrap(W.QLineEdit, "setPlaceholderText", (1,))
    _wrap(W.QPlainTextEdit, "setPlaceholderText", (1,))
    _wrap(W.QGroupBox, "setTitle", (1,))
    _wrap(W.QTabWidget, "setTabText", (2,))
    _wrap(W.QTabWidget, "addTab", (2,))
    _wrap(W.QTabWidget, "setTabToolTip", (2,))
    _wrap(W.QStatusBar, "showMessage", (1,))
    _wrap(W.QAction, "setText", (1,))
    _wrap(W.QMenu, "addAction", (1,))
    _wrap(W.QMenu, "addMenu", (1,))
    _wrap(W.QMessageBox, "setText", (1,))
    _wrap(W.QMessageBox, "setInformativeText", (1,))
    _wrap(W.QComboBox, "addItem", (1,))
    _wrap(W.QFormLayout, "addRow", (1,))
    _wrap(W.QSpinBox, "setSuffix", (1,))
    _wrap(W.QDoubleSpinBox, "setSuffix", (1,))
    orig_add = W.QListWidget.addItem
    W.QListWidget.addItem = lambda self, x, *a: orig_add(self, tr_exact(x) if isinstance(x, str) else x, *a)
    orig_set = W.QTableWidget.setItem

    def _set_item(self, r, c, item):
        if item is not None and CYR.search(item.text() or ""):
            item.setText(tr(item.text()))
        return orig_set(self, r, c, item)
    W.QTableWidget.setItem = _set_item
    for name in ("information", "warning", "critical", "question"):
        _wrap_static(W.QMessageBox, name, (1, 2))
    _wrap_static(W.QInputDialog, "getText", (1, 2))
    _wrap_static(W.QInputDialog, "getItem", (1, 2))
    _wrap_static(W.QInputDialog, "getInt", (1, 2))
    for name in ("getOpenFileName", "getSaveFileName", "getExistingDirectory", "getOpenFileNames"):
        _wrap_static(W.QFileDialog, name, (1,))
    orig_hdr = W.QTreeWidget.setHeaderLabels
    W.QTreeWidget.setHeaderLabels = lambda self, labels: orig_hdr(self, [tr(x) for x in labels])
    orig_thdr = W.QTableWidget.setHorizontalHeaderLabels
    W.QTableWidget.setHorizontalHeaderLabels = lambda self, labels: orig_thdr(self, [tr(x) for x in labels])

    class _Polish(C.QObject):
        def eventFilter(self, obj, ev):
            if ev.type() == C.QEvent.Polish and isinstance(obj, W.QWidget):
                translate_widget(obj)
            return False

    app._i18n_filter = _Polish(app)
    app.installEventFilter(app._i18n_filter)


if __name__ == "__main__":
    # Самопроверка: точное совпадение, шаблон с перестановкой, многострочный текст.
    EXACT["Бэкап"] = "Backup"
    rx, pre = _compile("Найдено {0} файлов в {1}")
    _PATTERNS.setdefault(pre[:3], []).append((rx, "Found {0} files in {1}", False))
    rx, pre = _compile("{0} в {1}")
    _WILD.append((rx, "{0} at {1}", True))
    LANG = "en"
    assert tr("Бэкап") == "Backup"
    assert tr("Найдено 5 файлов в C:\\Docs") == "Found 5 files in C:\\Docs"
    assert tr("Бэкап\nБэкап") == "Backup\nBackup"
    assert tr("Нет перевода") == "Нет перевода"
    assert tr("abc") == "abc"
    assert tr("MAC в любом виде") == "MAC в любом виде"   # weak template does not mangle phrases
    assert tr("Бэкап в Бэкап") == "Backup at Backup"
    assert _compile("a {{x}} {0}")[0].match("a {x} 1")
    print("i18n self-check OK")

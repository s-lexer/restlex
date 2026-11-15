"""Выбор языка при первом запуске: локаль — только для новой установки."""
import os, sys, tempfile
from pathlib import Path

os.environ["ProgramData"] = tempfile.mkdtemp()
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import restlex  # noqa: E402


class FakeCfg:
    def __init__(self, g=None, pr=None):
        self._g, self._pr = g or {}, pr or {}

    def gg(self, k, d=None):
        return self._g.get(k, d)


def test_fresh_install_follows_locale():
    assert restlex._default_language(FakeCfg(), "en") == "en"
    assert restlex._default_language(FakeCfg({"api_token": "x"}), "en") == "en"  # конфиг, созданный агентом


def test_existing_install_stays_russian():
    assert restlex._default_language(FakeCfg(pr={"a": {}}), "en") == "ru"
    assert restlex._default_language(FakeCfg({"report_url": "http://x"}), "en") == "ru"

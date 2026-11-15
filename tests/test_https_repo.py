"""REST HTTPS regression without opening owner's desktop or real profiles."""
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

os.environ["QT_QPA_PLATFORM"] = "offscreen"
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from PyQt5.QtWidgets import (
    QApplication, QCheckBox, QLabel, QLineEdit, QListWidget,
    QSpinBox, QStackedWidget, QWidget,
)
import restlex


class FakeRepoGui:
    def __init__(self):
        names = ("rest_host", "rest_path", "rest_user", "rest_pass",
                 "rest_scheme", "rest_url", "val")
        self._rpages = {"rest": {name: QLineEdit() for name in names}}
        self._repo_field_keys = lambda rt: restlex.RestlexUI._repo_field_keys(self, rt)
        self._repo_data_from_profile = lambda prof: restlex.RestlexUI._repo_data_from_profile(self, prof)
        self._upd_rest_url = lambda: restlex.RestlexUI._upd_rest_url(self)
        self._rtype = lambda: "rest"
        self._val_addr = lambda: None
        self._mark_repo_saved = lambda: None
        self.pid = "test"
        self.rtype_list = QListWidget()
        self.rstack = QStackedWidget()
        for _ in range(max(12, restlex.RIDX["rest"] + 1)):
            self.rtype_list.addItem("repo")
            self.rstack.addWidget(QWidget())
        self.edit_pw = QLineEdit()
        self.edit_wol_mac = QLineEdit()
        self.edit_wol_host = QLineEdit()
        self.edit_shutdown_mac = QLineEdit()
        self.chk_wol = QCheckBox()
        self.chk_wol_wait = QCheckBox()
        self.chk_shutdown_after = QCheckBox()
        self.chk_shutdown_remote = QCheckBox()
        self.spin_wol_interval = QSpinBox()
        self.spin_wol_timeout = QSpinBox()
        self.spin_shutdown_delay = QSpinBox()
        self._wol_sub = QWidget()
        self.lbl_wol_status = QLabel()
        self.lbl_sd_status = QLabel()
        self.lbl_chk = QLabel()

    def load(self, profile):
        restlex.RestlexUI._load_repo_fields(self, profile)

    def save_data(self):
        return restlex.RestlexUI._repo_data_from_ui(self)

    def parse(self, url):
        self._rpages["rest"]["rest_url"].setText(url)
        restlex.RestlexUI._parse_rest_url(self, self._rpages["rest"])
        self._upd_rest_url()

    def url(self):
        return self._rpages["rest"]["rest_url"].text()


class HttpsRepositoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.app = QApplication.instance() or QApplication([])

    def test_https_survives_save_and_reload(self):
        a = FakeRepoGui()
        a.parse("rest:https://example.test/repo")
        profile = a.save_data()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "profile.json"
            path.write_text(json.dumps(profile), encoding="utf-8")
            b = FakeRepoGui()
            b.load(json.loads(path.read_text(encoding="utf-8")))
            self.assertEqual("rest:https://example.test/repo", b.url())
            self.assertEqual("https", b._rpages["rest"]["rest_scheme"].text())
            b._rpages["rest"]["rest_path"].setText("/different")
            b._upd_rest_url()
            self.assertEqual("rest:https://example.test/different", b.url())

    def test_legacy_https_without_scheme(self):
        b = FakeRepoGui()
        b.load({"repo_type": "rest", "repo_path": "rest:https://example.test/old",
                "rest_host": "example.test", "rest_path": "/old"})
        self.assertEqual("rest:https://example.test/old", b.url())

    def test_legacy_http_stays_http(self):
        b = FakeRepoGui()
        b.load({"repo_type": "rest", "repo_path": "rest:http://example.test/old",
                "rest_host": "example.test", "rest_path": "/old"})
        self.assertEqual("rest:http://example.test/old", b.url())


if __name__ == "__main__":
    unittest.main()

"""Изолированная проверка lifecycle heartbeat: после stop файл не воскресает."""
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from restlex import BackupHeartbeat


class HeartbeatTests(unittest.TestCase):
    def test_stop_prevents_late_write(self):
        with tempfile.TemporaryDirectory() as tmp:
            hb = BackupHeartbeat("test-repository", "test-profile")
            hb.DIR = Path(tmp)
            hb.path = Path(tmp) / "heartbeat.json"
            hb.start()
            self.assertTrue(hb.path.exists())
            hb.stop()
            self.assertFalse(hb.path.exists())
            hb._write("running")  # simulated delayed writer
            self.assertFalse(hb.path.exists())


if __name__ == "__main__":
    unittest.main()

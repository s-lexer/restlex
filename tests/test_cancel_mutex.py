"""Отмена не освобождает межпроцессный mutex, пока restic ещё жив."""
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restlex


class CancelMutexTests(unittest.TestCase):
    def test_failed_process_stop_keeps_mutex(self):
        proc = Mock()
        proc.state.return_value = restlex.QProcess.Running
        proc.waitForFinished.return_value = False
        worker = SimpleNamespace(proc=proc)
        self.assertFalse(restlex.ResticWorker.stop(worker, wait_ms=1))
        proc.close.assert_not_called()

        worker.stop = Mock(return_value=False)
        ui = SimpleNamespace(
            _lock_preflight=None, _workers={"profile": worker},
            _backup_pid="profile", pid="profile", _log=Mock(), _bk_ui=Mock(),
            _cancel_backup=Mock(),
            btn_bk=SimpleNamespace(setEnabled=Mock()), _release_repo_mutex=Mock(),
        )
        with patch.object(restlex, "QTimer", SimpleNamespace(singleShot=Mock())) as timer:
            restlex.RestlexUI._cancel_backup(ui)
        ui._release_repo_mutex.assert_not_called()
        ui.btn_bk.setEnabled.assert_called_with(False)
        timer.singleShot.assert_called_once()


if __name__ == "__main__":
    unittest.main()

"""Безопасная проверка политики restic-lock без сети и без настоящего restic."""
import socket
import sys
import unittest
from pathlib import Path
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from restlex import LockPreflight


class LockSafetyTests(unittest.TestCase):
    def setUp(self):
        self.lp = LockPreflight("restic", "dummy-repo", "dummy-password")
        self.events = []
        self.lp.done.connect(lambda code, text: self.events.append((code, text)))
        self.lp._unlock = Mock(return_value=0)

    def check_lock(self, host, pid_state, age="2020-01-01T00:00:00Z"):
        self.lp._pid_is_restic = lambda pid: pid_state
        return self.lp._decide(
            {"hostname": host, "pid": 1234, "time": age},
            socket.gethostname())[0]

    def test_live_local_backup_never_unlocks(self):
        self.assertEqual("ABORT", self.check_lock(socket.gethostname(), True))

    def test_unknown_local_process_never_unlocks(self):
        self.assertEqual("WAIT", self.check_lock(socket.gethostname(), None))

    def test_dead_local_process_is_candidate(self):
        self.assertEqual("UNLOCK_FORCE", self.check_lock(socket.gethostname(), False))

    def test_remote_stale_lock_is_not_deleted(self):
        self.assertEqual("WAIT", self.check_lock("other-pc", False))

    def _run_with(self, locks, cat=None, next_locks=None, after_unlock=None, local_pid=False):
        reads = iter([locks, next_locks if next_locks is not None else locks,
                      after_unlock if after_unlock is not None else []])
        self.lp._list_locks = lambda: next(reads)
        self.lp._cat_lock = lambda lid: cat if cat is not None else None
        self.lp._pid_is_restic = lambda pid: local_pid
        self.lp._run_bg()

    def test_list_error_fails_closed(self):
        self._run_with(None)
        self.assertEqual(99, self.events[-1][0])
        self.lp._unlock.assert_not_called()

    def test_unreadable_lock_fails_closed(self):
        self._run_with(["abc"])
        self.assertEqual(99, self.events[-1][0])
        self.lp._unlock.assert_not_called()

    def test_foreign_lock_cannot_trigger_remove_all(self):
        self._run_with(["abc"], {"hostname": "elsewhere", "pid": 999, "time": "2000-01-01T00:00:00Z"})
        self.assertEqual(2, self.events[-1][0])
        self.lp._unlock.assert_not_called()

    def test_local_orphan_requires_stable_lock_set(self):
        self._run_with(["abc"], {"hostname": socket.gethostname(), "pid": 1234}, next_locks=["abc", "new"])
        self.assertEqual(2, self.events[-1][0])
        self.lp._unlock.assert_not_called()

    def test_confirmed_local_orphan_can_unlock(self):
        self._run_with(["abc"], {"hostname": socket.gethostname(), "pid": 1234})
        self.assertEqual(0, self.events[-1][0])
        self.lp._unlock.assert_called_once_with()

    def test_new_lock_after_unlock_blocks_backup(self):
        self._run_with(["abc"], {"hostname": socket.gethostname(), "pid": 1234},
                       after_unlock=["new"])
        self.assertEqual(2, self.events[-1][0])
        self.lp._unlock.assert_called_once_with()

    def test_cancelled_preflight_does_not_unlock(self):
        self.lp.cancel()
        self._run_with(["abc"], {"hostname": socket.gethostname(), "pid": 1234})
        self.assertEqual(2, self.events[-1][0])
        self.lp._unlock.assert_not_called()


if __name__ == "__main__":
    unittest.main()

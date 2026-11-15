"""Sleep-on-LAN: сообщения не приписывают ping статусу другого MAC."""
import sys
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from restlex import sleep_on_lan_probe_message


class ProbeMessageTests(unittest.TestCase):
    def test_reachable_ip_is_only_observation(self):
        m = sleep_on_lan_probe_message("test", "AA:BB:CC:DD:EE:FF", "192.0.2.5", True)
        self.assertIn("отвечает на ping", m)
        self.assertIn("не подтверждены", m)
        self.assertNotIn("не выключился", m)

    def test_unreachable_ip_is_not_confirmed_shutdown(self):
        m = sleep_on_lan_probe_message("test", "AA:BB:CC:DD:EE:FF", "192.0.2.5", False)
        self.assertIn("не отвечает на ping", m)
        self.assertIn("не подтверждены", m)
        self.assertNotIn("выключился успешно", m)


if __name__ == "__main__":
    unittest.main()

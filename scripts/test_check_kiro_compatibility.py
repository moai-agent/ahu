"""Provider-free regressions for live-check cleanup failure paths."""
import importlib.util
from pathlib import Path
import subprocess
import unittest

spec = importlib.util.spec_from_file_location('kiro_check', Path(__file__).with_name('check-kiro-compatibility.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class CleanupTests(unittest.TestCase):
    def fake(self, cancel=None, final=None, inventory=None):
        calls = []

        def ahu(action, *args, **kwargs):
            calls.append((action, *args))
            if action == 'tasks':
                return inventory if inventory is not None else {'tasks': [{'task_id': 'owned'}], 'unreadable': []}
            if action == 'result':
                return {'outcome': 'running'}
            if action == 'cancel':
                return cancel if cancel is not None else {'cancellation': 'requested'}
            if isinstance(final, Exception):
                raise final
            return final if final is not None else {'outcome': 'cancelled'}
        return ahu, calls

    def test_discovers_task_when_failed_launch_returned_no_id(self):
        ahu, calls = self.fake()
        result = module.cleanup_tasks(ahu, [])
        self.assertEqual(result[0]['status'], 'terminal_confirmed')
        self.assertIn(('cancel', 'owned'), calls)
        self.assertIn(('wait', 'owned'), calls)

    def test_cancel_error_and_unconfirmed_wait_fail_cleanup(self):
        for cancel, final in [({'error': 'failed'}, {'outcome': 'running'}),
                              ({'cancellation': 'requested'}, {'outcome': 'running'}),
                              ({'cancellation': 'requested'}, {'outcome': 'interrupted'}),
                              ({'cancellation': 'requested'}, subprocess.TimeoutExpired('wait', 1))]:
            with self.subTest(cancel=cancel, final=final):
                ahu, calls = self.fake(cancel=cancel, final=final)
                result = module.cleanup_tasks(ahu, ['owned'])
                self.assertEqual(result[0]['status'], 'cleanup_unconfirmed')
                self.assertEqual(calls.count(('cancel', 'owned')), 1)
                self.assertIn(('wait', 'owned'), calls)

    def test_unreadable_inventory_prevents_cleanup_success(self):
        ahu, _ = self.fake(inventory={'tasks': [], 'unreadable': [{'error': 'unreadable'}]})
        self.assertEqual(module.cleanup_tasks(ahu, []), [{'status': 'inventory_unreadable'}])

    def test_terminal_race_is_confirmed_even_when_cancel_fails(self):
        ahu, _ = self.fake(cancel={'error': 'already stopped'}, final={'outcome': 'succeeded'})
        result = module.cleanup_tasks(ahu, [])
        self.assertEqual(result[0]['status'], 'terminal_confirmed')
        self.assertFalse(result[0]['cancellation_accepted'])


if __name__ == '__main__':
    unittest.main()

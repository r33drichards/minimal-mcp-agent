import contextlib
import io
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import provision_runner as target

class ProvisionTests(unittest.TestCase):
    def test_secret_file_private_and_removed(self):
        paths = []
        def call(stage, command, timeout):
            self.assertEqual(stage, 'runner-secret-create')
            path = Path(command[command.index('--from-file') + 1])
            paths.append(path)
            data = path.read_text()
            self.assertRegex(data, r'^RUNNER_API_TOKEN=[0-9a-f]{64}\n$')
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
            self.assertNotIn(data.split('=')[1].strip(), ' '.join(command))
            self.assertIn('--redeploy', command)
            self.assertEqual(command[command.index('--app') + 1], target.APP_ID)
        output = io.StringIO()
        with patch.object(target, 'inspect_app'), patch.object(target, 'run_cli', side_effect=call), contextlib.redirect_stdout(output):
            target.provision()
        self.assertFalse(paths[0].exists())
        self.assertNotRegex(output.getvalue(), r'[0-9a-f]{64}')

    def test_failure_removes_file_without_retry(self):
        paths = []
        def fail(stage, command, timeout):
            paths.append(Path(command[command.index('--from-file') + 1]))
            raise SystemExit('safe fixed failure')
        with patch.object(target, 'inspect_app'), patch.object(target, 'run_cli', side_effect=fail) as call:
            with self.assertRaises(SystemExit):
                target.provision()
        self.assertEqual(call.call_count, 1)
        self.assertFalse(paths[0].exists())

if __name__ == '__main__':
    unittest.main()

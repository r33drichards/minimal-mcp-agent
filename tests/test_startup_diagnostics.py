import importlib.util
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import diagnose_startup as diag

SECRET = 'fake-secret-must-not-appear'

class StartupTests(unittest.TestCase):
    def test_known_error_reports_only_label(self):
        raw=json.dumps([{'message':'Error: Set RUNNER_API_TOKEN to a random secret of at least 32 bytes '+SECRET}])
        result=diag.summarize_logs(raw)
        self.assertEqual(result, {'records':1,'signals':['runner-token-missing']})
        self.assertNotIn(SECRET,json.dumps(result))

    def test_unknown_log_with_secret_is_not_printed(self):
        self.assertEqual(diag.summarize_logs(json.dumps([{'message':SECRET}])), {'records':1,'signals':[]})

    def test_empty_logs(self):
        self.assertEqual(diag.summarize_logs(''),{'records':0,'signals':[]})

    def test_malformed_response_does_not_leak(self):
        for raw in [SECRET,json.dumps({'message':SECRET}),json.dumps([{'other':SECRET}])]:
            with self.assertRaises(SystemExit) as e:diag.summarize_logs(raw)
            self.assertNotIn(SECRET,str(e.exception))

    def test_unsafe_health_url_never_requested(self):
        with patch.object(diag.urllib.request,'urlopen') as opener:
            for url in ['http://a.wasmer.app','https://evil.example','https://x:y@a.wasmer.app','https://a.wasmer.app/?token='+SECRET]:
                self.assertEqual(diag.health_status({'url':url}),'unavailable')
            opener.assert_not_called()

if __name__=='__main__':unittest.main()

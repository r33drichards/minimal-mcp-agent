#!/usr/bin/env python3
"""Provision only the authorized runner token; never emit secret values."""
import os
import secrets
import tempfile
import time
from pathlib import Path
from deploy_wasmer import APP_ID, inspect_app, run_cli
from diagnose_startup import health_status, summarize_logs
import json

def provision():
    inspect_app()
    # File input takes the CLI's create-only path, refusing an existing name.
    # Wasmer 7.5 reads dotenv despite the help text describing JSON.
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / 'runner.env'
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, 'w') as stream:
            stream.write('RUNNER_API_TOKEN=' + secrets.token_hex(32) + '\n')
        run_cli('runner-secret-create', ['wasmer', 'app', 'secrets', 'create',
            '--app', APP_ID, '--from-file', str(path), '--non-interactive',
            '--quiet', '--redeploy'], 180)
    print('RUNNER_API_TOKEN securely retained in Wasmer; current app redeploy requested.', flush=True)

def main():
    provision()
    for attempt in range(12):
        status = health_status(inspect_app())
        print('Public health status: ' + str(status), flush=True)
        if status == 200:
            break
        time.sleep(10)
    result = run_cli('startup-log-read', ['wasmer', 'app', 'logs', APP_ID,
        '--from=-2m', '--max', '100', '--format', 'json'], 60)
    print('Recent startup log summary: ' + json.dumps(summarize_logs(result.stdout)))
    print('Recent logs may overlap prior versions; no MCP configuration changed.')

if __name__ == '__main__':
    main()

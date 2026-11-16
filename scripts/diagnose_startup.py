#!/usr/bin/env python3
"""Read existing app health/logs; print only fixed labels, never raw logs."""
import json
import urllib.error
import urllib.parse
import urllib.request

from deploy_wasmer import APP_ID, inspect_app, run_cli

SIGNATURES = {
    'runner-token-missing': 'Set RUNNER_API_TOKEN to a random secret',
    'runner-token-too-short': 'RUNNER_API_TOKEN must contain at least 32 bytes',
    'runner-token-whitespace': 'RUNNER_API_TOKEN must not contain whitespace',
    'mcp-config-missing': 'Set MCP_SERVERS_JSON or MCP_URL',
    'mcp-config-invalid': 'Invalid MCP_SERVERS_JSON',
    'model-name-missing': 'Set OPENAI_MODEL',
    'model-not-available': 'OPENAI_MODEL is not listed for this account',
    'chatgpt-token-missing': 'Set CHATGPT_ACCESS_TOKEN through',
    'chatgpt-token-empty': 'CHATGPT_ACCESS_TOKEN is empty',
    'chatgpt-login-missing': 'Not signed in.',
    'mcp-auth-required': 'Auth required',
    'dns-failure': 'dns error',
    'tls-failure': 'invalid peer certificate',
    'listener-started': 'POST /turn requires a bearer token',
}


def summarize_logs(raw):
    if not raw.strip():
        return {'records': 0, 'signals': []}
    try:
        records = json.loads(raw)
        if not isinstance(records, list):
            raise ValueError()
        messages = []
        for record in records:
            if not isinstance(record, dict) or not isinstance(record.get('message'), str):
                raise ValueError()
            messages.append(record['message'])
    except (ValueError, TypeError):
        raise SystemExit('Startup logs: unsupported response; raw output withheld') from None
    signals = [label for label, needle in SIGNATURES.items() if any(needle in message for message in messages)]
    return {'records': len(records), 'signals': signals}


def health_status(app):
    try:
        url = app['url']
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != 'https' or not (parsed.hostname or '').endswith('.wasmer.app') or parsed.username or parsed.password or parsed.query or parsed.fragment or parsed.path not in ('', '/'):
            raise ValueError()
        # No credential is sent with this public health probe.
        with urllib.request.urlopen(url.rstrip('/') + '/health', timeout=20) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code
    except (OSError, ValueError, KeyError, TypeError):
        return 'unavailable'


def main():
    app = inspect_app()
    print('Public health status: ' + str(health_status(app)), flush=True)
    result = run_cli('startup-log-read', ['wasmer', 'app', 'logs', APP_ID,
        '--from=-1h', '--max', '100', '--format', 'json'], 60)
    print('Recent startup log summary: ' + json.dumps(summarize_logs(result.stdout)))
    print('Logs cover the last hour and may include older versions; signatures require correlation. No app changes or deployment performed.')


if __name__ == '__main__':
    main()

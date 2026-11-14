#!/usr/bin/env python3
"""Deploy the tested artifact; never print CLI output (it can contain secrets)."""
import json
import os
import re
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request


def check_url(url):
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != 'https' or not (parsed.hostname or '').endswith('.wasmer.app') or parsed.username or parsed.password:
        raise SystemExit('Unexpected Wasmer deployment URL')
    return url.rstrip('/')


def verify(url, commit):
    # These probes never carry credentials and never invoke a model or MCP tool.
    for attempt in range(30):
        try:
            with urllib.request.urlopen(url + '/health', timeout=10) as response:
                health = json.load(response)
            if health.get('status') == 'ok' and health.get('commit') == commit:
                break
        except (OSError, ValueError):
            pass
        time.sleep(5)
    else:
        raise SystemExit('Deployment did not become healthy at the expected commit')
    try:
        request = urllib.request.Request(url + '/turn', data=b'{}', headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=10):
            raise SystemExit('Unauthenticated /turn was not rejected')
    except urllib.error.HTTPError as error:
        if error.code != 401:
            raise SystemExit('Unauthenticated /turn did not return 401') from None


def main():
    if not os.environ.get('WASMER_TOKEN'):
        raise SystemExit('Set the production environment secret WASMER_TOKEN to a newly rotated Wasmer deployment token')
    commit = os.environ.get('GITHUB_SHA', '')
    if not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise SystemExit('GITHUB_SHA must identify the tested commit')
    # Capture both streams: Wasmer diagnostics and JSON config may contain secrets.
    result = subprocess.run(['wasmer', 'deploy', '--non-interactive', '--no-wait', '--format', 'json'], capture_output=True, text=True, timeout=600)
    if result.returncode:
        raise SystemExit('Wasmer deploy failed; check the Wasmer dashboard and token permissions. CLI output withheld to protect credentials.')
    try:
        deployment = json.loads(result.stdout)
        url = check_url(deployment['url'])
        app_id = deployment['app']['id']
    except (ValueError, KeyError, TypeError):
        raise SystemExit('Could not read deployment identity; CLI output withheld') from None
    if app_id != 'da_nBBIbtXUDZZG':
        raise SystemExit('Unexpected deployed application ID')
    verify(url, commit)
    # Also verify the stable app route, not only the immutable version URL.
    result = subprocess.run(['wasmer', 'app', 'get', app_id, '--format', 'json'], capture_output=True, text=True, timeout=60)
    if result.returncode:
        raise SystemExit('Could not resolve live app URL; CLI output withheld')
    try:
        live = check_url(json.loads(result.stdout)['url'])
    except (ValueError, KeyError, TypeError):
        raise SystemExit('Could not read live app URL; CLI output withheld') from None
    verify(live, commit)
    summary = f'Deployed commit `{commit}`. Healthy version: {url}; live app: {live}. Unauthenticated /turn returned 401.\n'
    print(summary)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as stream:
            stream.write(summary)


if __name__ == '__main__':
    main()

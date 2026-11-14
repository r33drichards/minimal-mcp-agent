# Build for Wasmer / WASIX

The native CLI and the hosted WASIX binary use the same agent loop. Browser OAuth
and Unix credential storage compile only on native Unix. WASIX requires a
`CHATGPT_ACCESS_TOKEN` supplied through Wasmer Secrets.

Verified toolchain: Wasmer 7.5.0, cargo-wasix 0.1.34 (WASIX Rust 1.97), WASI SDK 34.
Use official installation instructions at https://wasix.org/docs/language-guide/rust/installation
and https://docs.wasmer.io/runtime/get-started/ .

```sh
# If the host clang lacks the WASM target, point this at the official WASI SDK:
export CC_wasm32_wasmer_wasi=/path/to/wasi-sdk/bin/clang
python3 scripts/build_wasix.py
wasmer run dist/minimal-mcp-agent.wasm -- --help
python3 tests/mcp_smoke.py --wasmer dist/minimal-mcp-agent.wasm
wasmer package build --check
```

The script copies only source inputs into ignored `.wasix-build/`, verifies the
pinned rmcp 3.10.0 crate archive SHA-256, and changes only its reqwest dependency
from 0.13 to 0.12 plus the renamed rustls feature. The WASIX registry currently
provides reqwest 0.12.22+wasix.1, not 0.13. The native Cargo.toml/Cargo.lock stay
unchanged. `wasix/Cargo.lock` pins the compatibility build. The final artifact is
`dist/minimal-mcp-agent.wasm`; neither it nor package output contains credentials.

## Deployment

After `wasmer login`, confirm the intended account with `wasmer whoami`.
`wasmer.toml` packages only the compiled module; it does not mount this checkout.
`app.yaml` starts `serve` behind HTTPS, without enabling database/volume features.

Before sending real requests, provision secrets through Wasmer's app Settings →
Environment Vars or the supported CLI secret mechanism:

- `RUNNER_API_TOKEN`: independent random secret (at least 32 bytes).
- `CHATGPT_ACCESS_TOKEN`: current direct Sign in with ChatGPT access token.
- `OPENAI_MODEL`: available model slug for that account.
- `MCP_URL`: mcp-js's HTTPS `/mcp` endpoint.
- `MCP_TOKEN`: mcp-js-compatible OAuth/JWT access token, if authentication enabled.
- Optional `MCP_SESSION_ID`: mcp-js's persistent logical JS session name.

Do not paste secrets into shell history, GitHub, app.yaml or this repo. Tokens
provided through env vars are not refreshed automatically; rotate and redeploy.
The application intentionally fails startup while required config is missing.

```sh
wasmer deploy --owner r33drichards --app-name minimal-mcp-agent
```

Check costs/plan before approving any upgrade. A successful package upload is
not evidence of a healthy app; verify `/health`, unauthorized `/turn` rejection,
and an authorized real turn after credentials are provisioned.

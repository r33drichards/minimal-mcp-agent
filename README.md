# Minimal MCP Agent

A small Rust terminal agent: **read input → ask the model → run MCP tools → repeat**.
The agent loop is in [`src/agent.rs`](src/agent.rs). No agent framework, database,
local Rust tool registry, or Codex subprocess. Tools come from one MCP server.

## Run

Requires Rust 1.88+ and a Unix-like system (macOS/Linux). Authentication uses the
official **Sign in with ChatGPT** direct flow, not an OpenAI API key and not copied
Codex credentials. Account/plan availability and app usage limits apply.

```sh
cargo build --locked
cargo run --locked -- login
cargo run --locked -- models

# Pick a slug from the account-specific models command:
export OPENAI_MODEL='<model-slug>'
export MCP_URL='http://127.0.0.1:8000/mcp'
# Optional, only if your MCP server requires a bearer token:
# read -rs MCP_TOKEN; export MCP_TOKEN

cargo run --locked -- tools   # check connectivity and list tools, no model call
cargo run --locked
```

`login` opens your browser for ChatGPT authorization. Nothing needs to be pasted
into the source code. Choose an existing saved account or add another on subsequent
logins. `logout` revokes the active refresh token and removes its local credentials;
registration metadata is retained for later sign-in.

Tool invocations execute automatically, with no per-call approval or human-in-the-loop
step. Each call logs its name and arguments. `/quit` or EOF exits between turns.
Conversation history is in memory only; starting a new process starts a new conversation.

## The loop

```text
messages = []
for each user input:
    messages += user input
    repeat (at most 20 model rounds):
        output = Responses(messages, discovered MCP tool schemas)
        messages += all output items
        print assistant text
        if no function calls: break
        for each function call:
            result = MCP tools/call (or an error result)
            messages += function_call_output with the original call_id
```

Multiple calls execute sequentially. Tool errors become results the model can
reason about. A network/tool timeout is **not retried automatically**: an action
may already have happened. Check the external system before requesting a retry.
The round limit stops runaway loops after completing the pending result batch.

## What lives where

- `src/agent.rs`: the loop, provider-independent via closures, and its tests.
- `src/main.rs`: terminal input and automatic MCP connection/discovery/calls.
- `src/model.rs`: Responses streaming and account-specific model listing.
- `src/auth.rs`: OAuth plumbing, separate so the core loop stays easy to read.

The official `rmcp` SDK handles Streamable HTTP/SSE, modern protocol discovery,
legacy initialization fallback, and paginated tool discovery. This starter does
not launch stdio servers or implement MCP OAuth; use `MCP_URL` plus an optional
`MCP_TOKEN`. Old two-endpoint HTTP+SSE servers are not supported. Tool schemas are
loaded at startup; restart after a server's tools change. MCP prompts/resources
are not loaded automatically.

MCP tool names are mapped to `mcp_0`, `mcp_1`, etc. for model compatibility, then
mapped back when invoked. Structured/text MCP results are retained as JSON tool
output. Images/audio are not converted into model image/audio inputs.

Responses use `store: false`, `stream: true`; full completed output (including
encrypted reasoning) is preserved before tool results. An interrupted, failed,
or incomplete stream is an error, not a successful assistant turn. This tiny
version buffers a completed response instead of rendering token-by-token.

## Security and limits

- Connect only to MCP servers you trust. Their tool descriptions and results go
  to the selected model, and their tools may perform real external actions.
- `MCP_TOKEN` is sent **only** to `MCP_URL`; the ChatGPT credential is sent only to
  official OpenAI auth/API endpoints. Credential-bearing redirects are disabled.
- MCP HTTP is allowed only on loopback; use HTTPS for remote servers.
- OAuth uses PKCE, fresh state/nonce, and signed ID-token validation. Credentials
  are saved outside the repo under `~/.config/minimal-mcp-agent/`, with owner-only
  permissions and serialized atomic refresh. The file is permission-protected,
  **not encrypted at rest**. Do not sync or share it.
- No shell/filesystem tools are bundled. **All discovered MCP tools are available
  to the model and execute without approval**, with the server's own permissions.
  Use a least-privilege MCP server and avoid exposing destructive or sensitive
  tools you do not want the model to call autonomously.
- No history persistence, compaction, automatic retry, background jobs, or
  parallel tool execution. Long conversations eventually hit model context limits.

## Check

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 tests/mcp_smoke.py
```

Unit tests use no credentials or paid model requests. End-to-end authentication
and real model inference require your own browser sign-in and eligible plan.

## Protocol references

- [Sign in with ChatGPT: registration](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
- [ChatGPT plan models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)
- [Official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)

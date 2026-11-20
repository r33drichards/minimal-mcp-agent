use anyhow::{Context, Result, anyhow, ensure};
use rmcp::{RoleClient, model::CallToolRequestParams, service::Peer};
use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, Function, Promise,
    function::{Async, Func},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    time::Duration,
};
use tokio::time::{Instant, timeout_at};

const TIME_LIMIT: Duration = Duration::from_secs(120);
const MAX_CODE_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_CALLS: usize = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunJs {
    code: String,
}

pub struct QuickJsTools {
    catalog: String,
    peers: Rc<BTreeMap<(String, String), Peer<RoleClient>>>,
}

impl QuickJsTools {
    pub fn new(
        servers: &[crate::config::McpServer],
        tools: &[(usize, rmcp::model::Tool)],
        connections: &[rmcp::service::RunningService<RoleClient, ()>],
    ) -> Self {
        let catalog = tools
            .iter()
            .map(|(index, tool)| {
                json!({
                    "server": servers[*index].name,
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                })
            })
            .collect::<Vec<_>>();
        let peers = tools
            .iter()
            .map(|(index, tool)| {
                (
                    (servers[*index].name.clone(), tool.name.to_string()),
                    connections[*index].peer().clone(),
                )
            })
            .collect();
        Self {
            catalog: json!(catalog).to_string(),
            peers: Rc::new(peers),
        }
    }

    pub fn definitions() -> Vec<Value> {
        vec![json!({
            "type": "function",
            "name": "run_js",
            "description": "Run JavaScript in a fresh sandbox. Use mcp.listTools() or mcp.listTools(server) for schemas, then await mcp.callTool(server, name, arguments). Use console.log to return results. Await all calls before finishing.",
            "parameters": {
                "type": "object", "properties": {"code": {"type": "string"}},
                "required": ["code"], "additionalProperties": false
            },
            "strict": true
        })]
    }

    pub async fn call(&self, name: &str, arguments: &str, report: bool) -> Result<Value> {
        ensure!(name == "run_js", "Unknown tool");
        let RunJs { code } = serde_json::from_str(arguments).context("Invalid run_js arguments")?;
        ensure!(
            !code.trim().is_empty() && code.len() <= MAX_CODE_BYTES,
            "Invalid run_js code length"
        );
        if report {
            println!("Tool: run_js");
        }

        let deadline = Instant::now() + TIME_LIMIT;
        let runtime = AsyncRuntime::new()?;
        runtime.set_memory_limit(32 * 1024 * 1024).await;
        runtime.set_max_stack_size(256 * 1024).await;
        runtime
            .set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)))
            .await;
        let context = AsyncContext::full(&runtime).await?;
        let calls = Rc::new(Cell::new(0usize));
        let pending = Rc::new(Cell::new(0usize));
        let uncertain = Rc::new(Cell::new(false));
        let output = Rc::new(RefCell::new(String::new()));
        let peers = self.peers.clone();
        let run = context.async_with(async |ctx| -> Result<()> {
            let list = ctx.json_parse(self.catalog.as_bytes())?;
            let pending_calls = pending.clone();
            let failed = uncertain.clone();
            let call = Func::from(Async(
                move |server: String, name: String, arguments: String| {
                    let peers = peers.clone();
                    let calls = calls.clone();
                    let pending = pending_calls.clone();
                    let uncertain = failed.clone();
                    async move {
                        let peer = peers
                            .get(&(server, name.clone()))
                            .ok_or_else(|| js_error("Unknown MCP tool"))?;
                        let args: Map<String, Value> = serde_json::from_str(&arguments)
                            .map_err(|_| js_error("MCP arguments must be an object"))?;
                        if uncertain.get() {
                            return Err(js_error("An earlier MCP call has an unknown outcome"));
                        }
                        if calls.get() >= MAX_CALLS || Instant::now() >= deadline {
                            return Err(js_error("MCP call limit reached"));
                        }
                        calls.set(calls.get() + 1);
                        pending.set(pending.get() + 1);
                        let result = peer
                            .call_tool(CallToolRequestParams::new(name).with_arguments(args))
                            .await;
                        pending.set(pending.get() - 1);
                        let result = result.map_err(|_| {
                            uncertain.set(true);
                            js_error("MCP call failed with an unknown outcome")
                        })?;
                        serde_json::to_string(&result)
                            .map_err(|_| js_error("Cannot encode MCP result"))
                    }
                },
            ));
            let captured_output = output.clone();
            let write = Func::from(move |line: String| -> rquickjs::Result<()> {
                let mut output = captured_output.borrow_mut();
                if line.len() >= MAX_OUTPUT_BYTES - output.len() {
                    return Err(js_error("Console output limit exceeded"));
                }
                output.push_str(&line);
                output.push('\n');
                Ok(())
            });
            let install: Function = ctx
                .eval(
                    r#"
                (catalog, call, write) => {
                    const parse = JSON.parse;
                    const stringify = JSON.stringify;
                    const toString = String;
                    globalThis.mcp = {
                        listTools: (server) => parse(stringify(catalog.filter(tool =>
                            server === undefined || tool.server === server))),
                        callTool: async (server, name, args = {}) => {
                            const result = parse(await call(server, name, stringify(args)));
                            if (result.isError) {
                                const error = new Error("MCP tool returned an error");
                                error.result = result;
                                throw error;
                            }
                            return result;
                        },
                    };
                    globalThis.console = {log: (...args) => write(args.map(value =>
                        typeof value === "string" ? value : stringify(value) ?? toString(value)
                    ).join(" "))};
                }
            "#,
                )
                .catch(&ctx)
                .map_err(|error| anyhow!("{error}"))?;
            install
                .call::<_, ()>((list, call, write))
                .catch(&ctx)
                .map_err(|error| anyhow!("{error}"))?;
            let promise: Promise = ctx
                .eval(format!("(async () => {{\n{code}\n}})()"))
                .catch(&ctx)
                .map_err(|error| anyhow!("{error}"))?;
            promise
                .into_future::<()>()
                .await
                .catch(&ctx)
                .map_err(|error| anyhow!("{error}"))
        });
        timeout_at(deadline, run)
            .await
            .context("run_js timed out")??;
        ensure!(pending.get() == 0, "run_js finished with pending MCP calls");
        ensure!(!uncertain.get(), "An MCP tool call has an unknown outcome");
        let output = output.take();
        Ok(Value::String(output))
    }
}

fn js_error(message: &str) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message("script", "MCP bridge", message)
}

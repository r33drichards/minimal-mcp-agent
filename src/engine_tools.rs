use crate::config::{self, McpServer};
use anyhow::{Context, Result, ensure};
use mcp_v8_uniffi::{Engine, ExecutionRequest, UpstreamMcpServer};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{runtime::Runtime, time::timeout};

const EXECUTION_LIMIT: Duration = Duration::from_secs(120);

pub struct McpTools {
    engine: Arc<Engine>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunJs {
    code: String,
}

impl McpTools {
    pub fn connect(_runtime: &Runtime, servers: &[McpServer]) -> Result<Self> {
        let upstream = servers
            .iter()
            .map(|server| -> Result<UpstreamMcpServer> {
                Ok(UpstreamMcpServer {
                    name: server.name.clone(),
                    url: server.url.clone(),
                    bearer_token: config::bearer(server)?,
                    session_id: config::session_id(server)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let engine = Engine::create_stateless_with_mcp(128, EXECUTION_LIMIT.as_secs(), upstream)?;
        Ok(Self { engine })
    }

    pub fn list(&self, servers: &[McpServer]) -> Result<()> {
        for server in servers {
            for tool in self.engine.list_upstream_tools(server.name.clone())? {
                if servers.len() == 1 {
                    println!("{}", tool.name);
                } else {
                    println!("{}: {}", server.name, tool.name);
                }
            }
        }
        Ok(())
    }

    pub fn definitions(&self, _servers: &[McpServer]) -> Vec<Value> {
        vec![json!({
            "type": "function",
            "name": "run_js",
            "description": "Run JavaScript in a fresh sandbox. Discover configured MCP tools with mcp.servers and mcp.listTools(server). Call them with await mcp.callTool(server, name, arguments). Use console.log to return useful data.",
            "parameters": {
                "type": "object",
                "properties": {"code": {"type": "string"}},
                "required": ["code"],
                "additionalProperties": false
            },
            "strict": true
        })]
    }

    pub fn call(
        &self,
        runtime: &Runtime,
        name: &str,
        arguments: &str,
        hosted: bool,
    ) -> Result<Value> {
        ensure!(name == "run_js", "Unknown tool");
        let RunJs { code } = serde_json::from_str(arguments).context("Invalid run_js arguments")?;
        ensure!(!code.trim().is_empty(), "run_js requires code");
        if !hosted {
            println!("Tool: run_js");
        }
        runtime.block_on(async {
            let id = self
                .engine
                .submit_execution(ExecutionRequest {
                    code,
                    file: None,
                    heap: None,
                    fs: None,
                    session: None,
                    heap_memory_max_mb: None,
                    execution_timeout_secs: None,
                    tags: None,
                    mcp_headers: None,
                })
                .await?;
            let execution = match timeout(
                EXECUTION_LIMIT + Duration::from_secs(5),
                self.engine.clone().await_execution(id.clone()),
            )
            .await
            {
                Ok(result) => result?,
                Err(error) => {
                    let _ = self.engine.cancel_execution(id);
                    return Err(error).context("Waiting for run_js");
                }
            };
            ensure!(
                execution.status == "completed",
                "run_js {}: {}",
                execution.status,
                execution
                    .error
                    .as_deref()
                    .unwrap_or("execution did not complete")
            );
            let page = self
                .engine
                .get_execution_output(id, None, None, Some(0), Some(65_536))?;
            let mut output = page.data;
            if page.has_more {
                output.push_str("\n[Output truncated]");
            }
            Ok(Value::String(output))
        })
    }

    pub fn close(self, _runtime: &Runtime) -> Result<()> {
        self.engine.close()?;
        Ok(())
    }
}

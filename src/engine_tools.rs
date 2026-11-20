use crate::config::{self, McpServer};
use anyhow::{Context, Result, ensure};
use mcp_js::engine::{
    Engine, ExecutionRequest, HardeningConfig,
    execution::ExecutionRegistry,
    initialize_v8,
    mcp_client::{HttpClientOptions, McpClientManager, McpServerConfig, McpServerTransport},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{runtime::Runtime, time::timeout};

const EXECUTION_LIMIT: Duration = Duration::from_secs(120);

pub struct McpTools {
    engine: Arc<Engine>,
    _data: tempfile::TempDir,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunJs {
    code: String,
}

impl McpTools {
    pub fn connect(runtime: &Runtime, servers: &[McpServer]) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut configs = Vec::with_capacity(servers.len());
        let mut options = HashMap::with_capacity(servers.len());
        for server in servers {
            configs.push(McpServerConfig {
                name: server.name.clone(),
                transport: McpServerTransport::Http {
                    url: server.url.clone(),
                },
                auth: None,
            });
            options.insert(
                server.name.clone(),
                HttpClientOptions {
                    client: client.clone(),
                    bearer_token: config::bearer(server)?,
                    headers: config::session_header(server)?.into_iter().collect(),
                },
            );
        }
        let manager = runtime
            .block_on(async {
                timeout(
                    Duration::from_secs(30 * servers.len() as u64),
                    McpClientManager::connect_with_http_options(configs, options),
                )
                .await
            })?
            .map_err(anyhow::Error::msg)?;
        initialize_v8();
        let data = tempfile::tempdir()?;
        let path = data
            .path()
            .to_str()
            .context("Engine data path must be UTF-8")?;
        let registry = ExecutionRegistry::new(path).map_err(anyhow::Error::msg)?;
        let engine = Engine::from_engine(
            Engine::new_stateless(128 * 1024 * 1024, EXECUTION_LIMIT.as_secs(), 1)
                .with_hardening(HardeningConfig::all())
                .with_execution_registry(Arc::new(registry))
                .with_mcp_client_manager(manager),
        );
        Ok(Self {
            engine,
            _data: data,
        })
    }

    pub fn list(&self, servers: &[McpServer]) -> Result<()> {
        let manager = self
            .engine
            .mcp_client_manager()
            .context("Engine MCP manager unavailable")?;
        for server in servers {
            for tool in manager
                .list_tools(Some(&server.name))
                .map_err(anyhow::Error::msg)?
            {
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

    pub fn close(self, runtime: &Runtime) -> Result<()> {
        runtime.block_on(self.engine.shutdown());
        Ok(())
    }
}

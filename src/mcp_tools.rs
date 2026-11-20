use crate::config;
use anyhow::{Context, Result};
use config::McpServer;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::runtime::Runtime;

use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    model::{CallToolRequestParams, ProtocolVersion},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type Connection = rmcp::service::RunningService<rmcp::RoleClient, ()>;

pub struct McpTools {
    connections: Vec<Connection>,
    names: Vec<(usize, String)>,
    registry: BTreeMap<String, (usize, rmcp::model::Tool)>,
}

impl McpTools {
    pub fn connect(runtime: &Runtime, servers: &[McpServer]) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut connections = Vec::new();
        let mut discovered = Vec::new();
        for (server_index, server) in servers.iter().enumerate() {
            let mut transport_config =
                StreamableHttpClientTransportConfig::with_uri(server.url.clone());
            if let Some(header) = config::bearer(server)? {
                transport_config = transport_config.auth_header(header);
            }
            if let Some(session) = config::session_id(server)? {
                use reqwest::header::{HeaderName, HeaderValue};
                let mut value =
                    HeaderValue::from_str(&session).context("Invalid MCP session ID")?;
                value.set_sensitive(true);
                transport_config
                    .custom_headers
                    .insert(HeaderName::from_static("x-mcp-session-id"), value);
            }
            let mcp = runtime.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(30),
                    ().serve_with_lifecycle(
                        StreamableHttpClientTransport::with_client(
                            client.clone(),
                            transport_config,
                        ),
                        ClientLifecycleMode::Auto {
                            preferred_versions: vec![ProtocolVersion::LATEST],
                            legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
                        },
                    ),
                )
                .await
            })??;
            let tools = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(30), mcp.list_all_tools()).await
            })??;
            discovered.extend(tools.into_iter().map(|tool| (server_index, tool)));
            connections.push(mcp);
        }
        let names = discovered
            .iter()
            .map(|(index, tool)| (*index, tool.name.to_string()))
            .collect();
        // Stable aliases keep arbitrary MCP names compatible with model tool-name restrictions.
        let registry: BTreeMap<String, _> = discovered
            .into_iter()
            .map(|tool| (tool_alias(&servers[tool.0].name, &tool.1.name), tool))
            .collect();
        Ok(Self {
            connections,
            names,
            registry,
        })
    }

    pub fn list(&self, servers: &[McpServer]) -> Result<()> {
        for (index, name) in &self.names {
            if servers.len() == 1 {
                println!("{name}");
            } else {
                println!("{}: {name}", servers[*index].name);
            }
        }
        Ok(())
    }

    pub fn definitions(&self, servers: &[McpServer]) -> Vec<Value> {
        self.registry.iter().map(|(alias, (server_index, tool))|json!({
        "type":"function", "name":alias, "description":format!("{}/{}: {}", servers[*server_index].name, tool.name, tool.description.as_deref().unwrap_or("")),
        "parameters":tool.input_schema, "strict":false })).collect()
    }

    pub fn call(&self, runtime: &Runtime, alias: &str, args: &str, hosted: bool) -> Result<Value> {
        let (server_index, tool) = self.registry.get(alias).context("Unknown MCP tool")?;
        let args: Value = serde_json::from_str(args).context("Invalid tool arguments")?;
        let arguments = args
            .as_object()
            .context("Tool arguments must be an object")?
            .clone();
        if !hosted {
            println!("Tool: {}\nArguments: {}", tool.name, args);
        }
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(120),
                self.connections[*server_index].call_tool(
                    CallToolRequestParams::new(tool.name.to_string()).with_arguments(arguments),
                ),
            )
            .await
        })??;
        Ok(serde_json::to_value(result)?)
    }

    pub fn close(self, runtime: &Runtime) -> Result<()> {
        for mcp in self.connections {
            runtime.block_on(mcp.cancel())?;
        }
        Ok(())
    }
}

fn tool_alias(server: &str, tool: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(server.as_bytes());
    digest.update([0]);
    digest.update(tool.as_bytes());
    // Stable across discovery ordering, additions and process restarts.
    format!("mcp_{:x}", digest.finalize())[..64].to_string()
}

#[cfg(test)]
mod tool_identity_tests {
    use super::tool_alias;
    #[test]
    fn aliases_are_stable_namespaced_and_bounded() {
        assert_eq!(tool_alias("one", "exec"), tool_alias("one", "exec"));
        assert_ne!(tool_alias("one", "exec"), tool_alias("two", "exec"));
        assert_ne!(tool_alias("ab", "c"), tool_alias("a", "bc"));
        assert_eq!(tool_alias("one", &"a".repeat(500)).len(), 64);
    }
}

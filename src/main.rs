use minimal_mcp_agent::agent;
#[cfg(unix)]
mod auth;
#[cfg(not(unix))]
#[path = "auth_headless.rs"]
mod auth;
mod config;
mod model;
mod server;

use anyhow::{Context, Result, bail};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    model::{CallToolRequestParams, ProtocolVersion},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    io::{self, Write},
    time::Duration,
};

fn main() -> Result<()> {
    match env::args().nth(1).as_deref() {
        Some("login") => return auth::login(),
        Some("logout") => return auth::logout(),
        Some("models") => {
            for m in model::models(&config::chatgpt_token()?)? {
                println!(
                    "{}\t{}",
                    m["slug"].as_str().unwrap_or(""),
                    m["display_name"].as_str().unwrap_or("")
                );
            }
            return Ok(());
        }
        Some("tools" | "serve") => {}
        Some("--help" | "-h") => {
            println!(
                "minimal-mcp-agent [login|logout|models|tools|serve]\nSet OPENAI_MODEL and MCP_URL, then run with no arguments.\nOptional MCP_TOKEN: bearer token for your MCP server (not your ChatGPT token).\nTool calls execute automatically; /quit exits."
            );
            return Ok(());
        }
        Some(other) => bail!("Unknown command: {other}"),
        None => {}
    }
    let hosted = env::args().nth(1).as_deref() == Some("serve");
    if hosted {
        server::validate_environment()?;
    }
    let servers = config::mcp_servers()?;
    let runtime = tokio::runtime::Runtime::new()?;
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
        if let Some(key) = &server.session_id_env {
            let session =
                env::var(key).with_context(|| format!("Missing session ID variable {key}"))?;
            if session.is_empty() {
                bail!("MCP session ID must not be empty");
            }
            transport_config.custom_headers.insert(
                reqwest::header::HeaderName::from_static("x-mcp-session-id"),
                reqwest::header::HeaderValue::from_str(&session)
                    .context("Invalid MCP session ID")?,
            );
        }
        let mcp = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(30),
                ().serve_with_lifecycle(
                    StreamableHttpClientTransport::with_client(client.clone(), transport_config),
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
    if env::args().nth(1).as_deref() == Some("tools") {
        for (server_index, tool) in &discovered {
            if servers.len() == 1 {
                println!("{}", tool.name);
            } else {
                println!("{}: {}", servers[*server_index].name, tool.name);
            }
        }
        for mcp in connections {
            runtime.block_on(mcp.cancel())?;
        }
        return Ok(());
    }
    let selected = env::var("OPENAI_MODEL")
        .context("Set OPENAI_MODEL (run `minimal-mcp-agent models` to list choices)")?;
    let token = config::chatgpt_token()?;
    if !model::models(&token)?.iter().any(|m| m["slug"] == selected) {
        bail!("OPENAI_MODEL is not listed for this account");
    }
    // Stable aliases keep arbitrary MCP names compatible with model tool-name restrictions.
    let registry: BTreeMap<String, _> = discovered
        .into_iter()
        .enumerate()
        .map(|(i, tool)| (format!("mcp_{i}"), tool))
        .collect();
    let definitions: Vec<_> = registry.iter().map(|(alias, (server_index, tool))|json!({
        "type":"function", "name":alias, "description":format!("{}/{}: {}", servers[*server_index].name, tool.name, tool.description.as_deref().unwrap_or("")),
        "parameters":tool.input_schema, "strict":false })).collect();
    let mut run = |input: &str, messages: &mut Vec<Value>| -> Result<()> {
        agent::run_turn(
            messages,
            input,
            20,
            |messages| model::prompt(&config::chatgpt_token()?, &selected, messages, &definitions),
            |alias, args| {
                let (server_index, tool) = registry.get(alias).context("Unknown MCP tool")?;
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
                        connections[*server_index].call_tool(
                            CallToolRequestParams::new(tool.name.to_string())
                                .with_arguments(arguments),
                        ),
                    )
                    .await
                })??;
                Ok(serde_json::to_value(result)?)
            },
            |text| {
                if !hosted {
                    println!("Agent: {text}");
                }
            },
        )
    };
    if hosted {
        server::serve(&mut run)?;
    } else {
        println!(
            "Connected: {} MCP tools. /quit exits. Tool calls execute automatically.",
            registry.len()
        );
        let mut messages = vec![];
        while let Some(input) = read_line("You: ")? {
            if input == "/quit" {
                break;
            }
            if input.is_empty() {
                continue;
            }
            if let Err(error) = run(&input, &mut messages) {
                eprintln!("Error: {error}");
            }
        }
    }
    for mcp in connections {
        runtime.block_on(mcp.cancel())?;
    }
    Ok(())
}

fn read_line(prompt: &str) -> Result<Option<String>> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end().to_string()))
}

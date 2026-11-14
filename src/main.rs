mod agent;
mod auth;
mod model;

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
            for m in model::models(&auth::access_token()?)? {
                println!(
                    "{}\t{}",
                    m["slug"].as_str().unwrap_or(""),
                    m["display_name"].as_str().unwrap_or("")
                );
            }
            return Ok(());
        }
        Some("tools") => {}
        Some("--help" | "-h") => {
            println!(
                "minimal-mcp-agent [login|logout|models|tools]\nSet OPENAI_MODEL and MCP_URL, then run with no arguments.\nOptional MCP_TOKEN: bearer token for your MCP server (not your ChatGPT token).\nTool calls execute automatically; /quit exits."
            );
            return Ok(());
        }
        Some(other) => bail!("Unknown command: {other}"),
        None => {}
    }
    let url = env::var("MCP_URL").context("Set MCP_URL to a Streamable HTTP endpoint")?;
    let parsed = reqwest::Url::parse(&url)?;
    if parsed.scheme() != "https"
        && !(parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
    {
        bail!("MCP_URL must use HTTPS, or HTTP on loopback for local development");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Do not put credentials in MCP_URL");
    }
    let runtime = tokio::runtime::Runtime::new()?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    if let Ok(token) = env::var("MCP_TOKEN") {
        config = config.auth_header(format!("Bearer {token}"));
    }
    let mcp = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(30),
            ().serve_with_lifecycle(
                StreamableHttpClientTransport::with_client(client, config),
                ClientLifecycleMode::Auto {
                    preferred_versions: vec![ProtocolVersion::LATEST],
                    legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
                },
            ),
        )
        .await
    })??;
    let discovered = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), mcp.list_all_tools()).await
    })??;
    if env::args().nth(1).as_deref() == Some("tools") {
        for tool in &discovered {
            println!("{}", tool.name);
        }
        runtime.block_on(mcp.cancel())?;
        return Ok(());
    }
    let selected = env::var("OPENAI_MODEL")
        .context("Set OPENAI_MODEL (run `minimal-mcp-agent models` to list choices)")?;
    let token = auth::access_token()?;
    if !model::models(&token)?.iter().any(|m| m["slug"] == selected) {
        bail!("OPENAI_MODEL is not listed for this account");
    }
    // Stable aliases keep arbitrary MCP names compatible with model tool-name restrictions.
    let registry: BTreeMap<String, _> = discovered
        .into_iter()
        .enumerate()
        .map(|(i, tool)| (format!("mcp_{i}"), tool))
        .collect();
    let definitions: Vec<_> = registry.iter().map(|(alias, tool)|json!({
        "type":"function", "name":alias, "description":format!("{}: {}", tool.name, tool.description.as_deref().unwrap_or("")),
        "parameters":tool.input_schema, "strict":false })).collect();
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
        let result = agent::run_turn(
            &mut messages,
            &input,
            20,
            |messages| model::prompt(&auth::access_token()?, &selected, messages, &definitions),
            |alias, args| {
                let tool = registry.get(alias).context("Unknown MCP tool")?;
                let args: Value = serde_json::from_str(args).context("Invalid tool arguments")?;
                let arguments = args
                    .as_object()
                    .context("Tool arguments must be an object")?
                    .clone();
                println!("Tool: {}\nArguments: {}", tool.name, args);
                let result = runtime.block_on(async {
                    tokio::time::timeout(
                        Duration::from_secs(120),
                        mcp.call_tool(
                            CallToolRequestParams::new(tool.name.to_string())
                                .with_arguments(arguments),
                        ),
                    )
                    .await
                })??;
                Ok(serde_json::to_value(result)?)
            },
            |text| println!("Agent: {text}"),
        );
        if let Err(error) = result {
            eprintln!("Error: {error}");
        }
    }
    runtime.block_on(mcp.cancel())?;
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

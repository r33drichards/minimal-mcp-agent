use minimal_mcp_agent::agent;
#[cfg(unix)]
mod auth;
#[cfg(not(unix))]
#[path = "auth_headless.rs"]
mod auth;
mod config;
#[cfg(feature = "embedded-engine")]
#[path = "engine_tools.rs"]
mod mcp_tools;
#[cfg(not(feature = "embedded-engine"))]
mod mcp_tools;
mod model;
mod server;

use anyhow::{Context, Result, bail};
use mcp_tools::McpTools;
use serde_json::Value;
use std::{
    env,
    io::{self, Write},
};

#[cfg(not(any(feature = "embedded-engine", feature = "direct-mcp")))]
compile_error!("Enable embedded-engine or direct-mcp");

fn main() -> Result<()> {
    let command = env::args().nth(1);
    match command.as_deref() {
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
        Some("migrate") => {
            #[cfg(feature = "mysql-backend")]
            {
                minimal_mcp_agent::durable_mysql::MySqlBackend::from_env()?.migrate()?;
                println!("MySQL durability schema ready");
                return Ok(());
            }
            #[cfg(not(feature = "mysql-backend"))]
            bail!("Rebuild with --features mysql-backend to migrate");
        }
        Some("tools" | "serve") => {}
        #[cfg(feature = "embedded-engine")]
        Some("run-js") => {}
        Some("--help" | "-h") => {
            println!(
                "minimal-mcp-agent [login|logout|models|tools|serve|migrate]\nSet OPENAI_MODEL and MCP_URL, then run with no arguments.\nOptional MCP_TOKEN: bearer token for your MCP server (not your ChatGPT token).\nTool calls execute automatically; /quit exits."
            );
            #[cfg(feature = "embedded-engine")]
            println!("run-js '<code>' runs JavaScript locally without a model call.");
            return Ok(());
        }
        Some(other) => bail!("Unknown command: {other}"),
        None => {}
    }
    let hosted = command.as_deref() == Some("serve");
    let durable = match env::var("DURABLE_BACKEND").as_deref() {
        Ok("mysql") => true,
        Ok("ephemeral") | Err(env::VarError::NotPresent) => false,
        _ => bail!("DURABLE_BACKEND must be ephemeral or mysql"),
    };
    if durable && !hosted {
        bail!("DURABLE_BACKEND=mysql requires serve mode with stable session_id/request_id");
    }
    #[cfg(not(feature = "mysql-backend"))]
    if durable {
        bail!("Rebuild with --features mysql-backend for durable serving");
    }
    #[cfg(feature = "mysql-backend")]
    let backend = if durable {
        Some(minimal_mcp_agent::durable_mysql::MySqlBackend::from_env()?)
    } else {
        None
    };
    if hosted {
        server::validate_environment()?;
    }
    let servers = config::mcp_servers()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let tools = McpTools::connect(&runtime, &servers)?;
    if command.as_deref() == Some("tools") {
        tools.list(&servers)?;
        return tools.close(&runtime);
    }
    #[cfg(feature = "embedded-engine")]
    if command.as_deref() == Some("run-js") {
        let code = env::args()
            .nth(2)
            .context("Usage: minimal-mcp-agent run-js '<code>'")?;
        let result = tools.call(
            &runtime,
            "run_js",
            &serde_json::json!({"code": code}).to_string(),
            true,
        );
        tools.close(&runtime)?;
        println!(
            "{}",
            result?
                .as_str()
                .context("Expected run_js text output")?
                .trim_end()
        );
        return Ok(());
    }
    let selected = env::var("OPENAI_MODEL")
        .context("Set OPENAI_MODEL (run `minimal-mcp-agent models` to list choices)")?;
    let token = config::chatgpt_token()?;
    if !model::models(&token)?.iter().any(|m| m["slug"] == selected) {
        bail!("OPENAI_MODEL is not listed for this account");
    }
    let definitions = tools.definitions(&servers);
    let mut prompt_llm = |messages: &[Value]| {
        model::prompt(&config::chatgpt_token()?, &selected, messages, &definitions)
    };
    let mut call_tool = |name: &str, args: &str| tools.call(&runtime, name, args, hosted);
    let mut print_text = |text: &str| {
        if !hosted {
            println!("Agent: {text}");
        }
    };
    #[cfg(feature = "mysql-backend")]
    if let Some(backend) = backend {
        server::serve_durable(|session_id, request_id, input| {
            minimal_mcp_agent::durable::run_durable_turn(
                &backend,
                minimal_mcp_agent::durable::Turn {
                    session_id,
                    request_id,
                    input,
                    max_rounds: 20,
                },
                &mut prompt_llm,
                &mut call_tool,
                &mut print_text,
                &mut |_, _: &agent::Checkpoint| Ok(()),
            )
        })?;
        return tools.close(&runtime);
    }
    let mut run = |input: &str, messages: &mut Vec<Value>| -> Result<()> {
        agent::run_turn(
            messages,
            input,
            20,
            &mut prompt_llm,
            &mut call_tool,
            &mut print_text,
        )
    };
    if hosted {
        server::serve(&mut run)?;
    } else {
        println!("Connected. /quit exits. Tool calls execute automatically.");
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
    tools.close(&runtime)
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

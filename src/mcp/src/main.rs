#[allow(dead_code)]
mod chunkers;
#[allow(dead_code)]
mod store;

use anyhow::Result;
use mcp_server::router::{CapabilitiesBuilder, RouterService};
use mcp_server::{ByteTransport, Router, Server};
use mcp_spec::content::Content;
use mcp_spec::handler::{PromptError, ResourceError, ToolError};
use mcp_spec::prompt::{Prompt, PromptArgument};
use mcp_spec::protocol::ServerCapabilities;
use mcp_spec::resource::Resource;
use mcp_spec::tool::Tool;
use serde_json::Value;
use std::{future::Future, pin::Pin};
use tokio::io::{stdin, stdout};

#[derive(Clone)]
struct KnowledgeBaeRouter;

impl KnowledgeBaeRouter {
    fn new() -> Self {
        Self
    }
}

impl Router for KnowledgeBaeRouter {
    fn name(&self) -> String {
        "knowledge-bae".to_string()
    }

    fn instructions(&self) -> String {
        "Knowledge Bae MCP Server - A tool for AI agents to ingest documentation from local git repositories".to_string()
    }

    fn capabilities(&self) -> ServerCapabilities {
        CapabilitiesBuilder::new()
            .with_tools(true)
            .with_resources(false, false)
            .with_prompts(true)
            .build()
    }

    fn list_tools(&self) -> Vec<Tool> {
        vec![Tool::new(
            "hello_world".to_string(),
            "A simple hello world tool".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        )]
    }

    fn call_tool(
        &self,
        tool_name: &str,
        _arguments: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Content>, ToolError>> + Send + 'static>> {
        let tool_name = tool_name.to_string();

        Box::pin(async move {
            match tool_name.as_str() {
                "hello_world" => Ok(vec![Content::text("Hello World from the tool!")]),
                _ => Err(ToolError::NotFound(format!("Tool {} not found", tool_name))),
            }
        })
    }

    fn list_resources(&self) -> Vec<Resource> {
        vec![]
    }

    fn read_resource(
        &self,
        uri: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ResourceError>> + Send + 'static>> {
        let uri = uri.to_string();
        Box::pin(async move {
            Err(ResourceError::NotFound(format!(
                "Resource {} not found",
                uri
            )))
        })
    }

    fn list_prompts(&self) -> Vec<Prompt> {
        vec![Prompt::new(
            "hello_username",
            Some("A prompt that greets the user by name"),
            Some(vec![PromptArgument {
                name: "username".to_string(),
                description: Some("The name of the user to greet".to_string()),
                required: Some(true),
            }]),
        )]
    }

    fn get_prompt(
        &self,
        prompt_name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, PromptError>> + Send + 'static>> {
        let prompt_name = prompt_name.to_string();
        Box::pin(async move {
            match prompt_name.as_str() {
                "hello_username" => {
                    // The prompt template - arguments would be filled in by the client
                    Ok("Hello {username}".to_string())
                }
                _ => Err(PromptError::NotFound(format!(
                    "Prompt {} not found",
                    prompt_name
                ))),
            }
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("Hello World from Knowledge Bae MCP Server!");

    // Create the router
    let router = KnowledgeBaeRouter::new();

    // Create the router service
    let service = RouterService(router);

    // Create server with stdio transport
    let transport = ByteTransport::new(stdin(), stdout());
    let server = Server::new(service);

    // Run the server
    server.run(transport).await?;

    Ok(())
}

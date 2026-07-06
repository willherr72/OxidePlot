//! OxidePlot MCP server — lets Claude drive OxidePlot over stdio: load a
//! dataset, understand it (stats / raw rows / a rendered image), and iterate.
//!
//! M2 skeleton: nails the `rmcp` 0.16 server API with a single `ping` tool. The
//! real data/graph tools are layered on top once this compiles + tools/list works.

use rmcp::{
    handler::server::tool::ToolRouter, handler::server::wrapper::Parameters, model::*, schemars,
    tool, tool_handler, tool_router, transport::stdio, ErrorData as McpError, ServerHandler,
    ServiceExt,
};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct PingParams {
    /// Any text; the server echoes it back.
    message: String,
}

/// The MCP server handler. Real state (loaded datasets, graph specs) is added
/// alongside the data/graph tools in the following tasks.
#[derive(Clone)]
struct OxidePlot {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl OxidePlot {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Connectivity check — echoes the message back.")]
    async fn ping(
        &self,
        Parameters(PingParams { message }): Parameters<PingParams>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "pong: {message}"
        ))]))
    }
}

#[tool_handler]
impl ServerHandler for OxidePlot {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "OxidePlot: load CSV/Excel data, describe and query it, and render plots to images."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = OxidePlot::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

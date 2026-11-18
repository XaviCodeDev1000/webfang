//! Tests extracted from the `server.rs` hub by topic (PR 1 of #2584, pure move).
//! Bodies are verbatim from the original inline test module.
//!
//! Shared base for the test children: the `McpHandler` harness.

use crate::mcp_server::{McpHandler, McpState};
use webfang_core::config::Config;
use webfang_core::di::{Container, ContainerExt};

/// Build a test McpHandler with DI container.
pub(super) async fn test_handler() -> McpHandler {
    let config = Config::default();
    let container = Container::from_config(config).await.unwrap();
    let state = McpState::new(container);
    McpHandler::new(state)
}

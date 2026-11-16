//! Tests extraídos del hub `server.rs` por tema (PR 1 de #2584, movimiento puro).
//! Los cuerpos son verbatim del módulo de tests inline original.
//!
//! Base compartida por los hijos de test: el harness de `McpHandler`.

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

use std::pin::Pin;
use async_trait::async_trait;
use futures::Stream;

use crate::llm::error::LlmError;
use crate::llm::types::{ChatMessage, StreamChunk, ToolDefinition};

pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<StreamChunk, LlmError>> + Send>>;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Nome identificador do provedor (ex: "google", "anthropic", "openai")
    fn provider_name(&self) -> &'static str;

    /// Nome do modelo ativo (ex: "gemini-2.0-flash")
    fn model_name(&self) -> &str;

    /// Inicia uma conclusão em streaming com suporte a mensagens, histórico e ferramentas
    async fn stream_completion(
        &self,
        system_instruction: Option<&str>,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<ChunkStream, LlmError>;
}

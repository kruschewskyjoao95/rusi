pub mod error;
pub mod gemini;
pub mod provider;
pub mod stream;
pub mod types;

pub use error::LlmError;
pub use gemini::GeminiProvider;
pub use provider::{ChunkStream, LlmProvider};
pub use stream::parse_gemini_sse_stream;
pub use types::{ChatMessage, ContentPart, Role, StreamChunk, TokenUsage, ToolCall, ToolDefinition, ToolResult};

use crate::config::Config;

/// Cria uma instância do provedor LLM apropriado com base no nome do modelo e na configuração ativa.
pub fn create_provider(model: &str, config: &Config) -> Result<Box<dyn LlmProvider>, LlmError> {
    let model_lower = model.to_lowercase();

    if model_lower.starts_with("gemini") {
        let api_key = config
            .api_keys
            .gemini_api_key()
            .ok_or_else(|| LlmError::MissingApiKey("Google Gemini (GEMINI_API_KEY)".to_string()))?;

        Ok(Box::new(GeminiProvider::new(api_key, model)))
    } else if model_lower.starts_with("claude") {
        Err(LlmError::UnsupportedModel(format!(
            "O modelo '{model}' (Anthropic Claude) será suportado nas próximas fases."
        )))
    } else if model_lower.starts_with("gpt") || model_lower.starts_with("o1") || model_lower.starts_with("o3") {
        Err(LlmError::UnsupportedModel(format!(
            "O modelo '{model}' (OpenAI) será suportado nas próximas fases."
        )))
    } else {
        // Fallback: se houver chave do Gemini e o usuário não especificou o prefixo, tenta Gemini
        if let Some(api_key) = config.api_keys.gemini_api_key() {
            Ok(Box::new(GeminiProvider::new(api_key, model)))
        } else {
            Err(LlmError::UnsupportedModel(format!(
                "Modelo '{model}' não reconhecido e nenhuma chave de API compatível encontrada."
            )))
        }
    }
}

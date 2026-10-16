pub mod request;
pub mod response;

use std::time::Duration;
use async_trait::async_trait;

use crate::llm::error::LlmError;
use crate::llm::gemini::request::GeminiRequest;
use crate::llm::provider::{ChunkStream, LlmProvider};
use crate::llm::stream::parse_gemini_sse_stream;
use crate::llm::types::{ChatMessage, ToolDefinition};

pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl GeminiProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();

        Self {
            client,
            api_key: api_key.into(),
            model: model.into(),
        }
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn provider_name(&self) -> &'static str {
        "google"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    async fn stream_completion(
        &self,
        system_instruction: Option<&str>,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<ChunkStream, LlmError> {
        if self.api_key.trim().is_empty() {
            return Err(LlmError::MissingApiKey("Google Gemini".to_string()));
        }

        let payload = GeminiRequest::build(system_instruction, messages, tools);

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.model
        );

        let response = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(LlmError::HttpError)?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Não foi possível ler o corpo de erro da API".to_string());
            return Err(LlmError::ApiError {
                status: status.as_u16(),
                message: error_text,
            });
        }

        let byte_stream = response.bytes_stream();
        Ok(parse_gemini_sse_stream(byte_stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{ChatMessage, ContentPart, Role};

    #[test]
    fn test_gemini_request_serialization() {
        let messages = vec![
            ChatMessage::user("Olá Gemini!"),
            ChatMessage {
                role: Role::Assistant,
                parts: vec![ContentPart::Text {
                    text: "Como posso ajudar?".to_string(),
                }],
            },
        ];

        let req = GeminiRequest::build(Some("Você é o Rusi."), &messages, &[]);
        let json = serde_json::to_string(&req).expect("deve serializar");

        assert!(json.contains("Olá Gemini!"));
        assert!(json.contains("Você é o Rusi."));
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"role\":\"model\""));
    }

    #[test]
    fn test_gemini_tool_definition_serialization() {
        let tool = ToolDefinition {
            name: "file_read".to_string(),
            description: "Lê o conteúdo de um arquivo".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
        };

        let req = GeminiRequest::build(None, &[ChatMessage::user("leia o main.rs")], &[tool]);
        let json = serde_json::to_string(&req).expect("deve serializar");

        assert!(json.contains("functionDeclarations"));
        assert!(json.contains("file_read"));
        assert!(json.contains("Lê o conteúdo de um arquivo"));
    }
}

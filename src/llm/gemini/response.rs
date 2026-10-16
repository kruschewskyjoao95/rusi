use serde::Deserialize;

use crate::llm::types::{StreamChunk, TokenUsage, ToolCall};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiStreamResponse {
    pub candidates: Option<Vec<GeminiCandidate>>,
    pub usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiCandidate {
    pub content: Option<GeminiResponseContent>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiResponseContent {
    pub parts: Option<Vec<GeminiResponsePart>>,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiResponsePart {
    pub text: Option<String>,
    pub function_call: Option<GeminiResponseFunctionCall>,
}

#[derive(Debug, Deserialize)]
pub struct GeminiResponseFunctionCall {
    pub name: String,
    pub args: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiUsageMetadata {
    pub prompt_token_count: Option<u32>,
    pub candidates_token_count: Option<u32>,
    pub total_token_count: Option<u32>,
}

impl GeminiStreamResponse {
    pub fn into_stream_chunks(self) -> Vec<StreamChunk> {
        let mut chunks = Vec::new();

        if let Some(candidates) = self.candidates {
            for cand in candidates {
                if let Some(content) = cand.content
                    && let Some(parts) = content.parts
                {
                    for part in parts {
                        if let Some(text) = part.text
                            && !text.is_empty()
                        {
                            chunks.push(StreamChunk::Text(text));
                        }
                        if let Some(fc) = part.function_call {
                            let call_id = format!("call_{}", fc.name);
                            chunks.push(StreamChunk::FunctionCall(ToolCall {
                                id: call_id,
                                name: fc.name,
                                arguments: fc.args.unwrap_or(serde_json::Value::Null),
                            }));
                        }
                    }
                }


                if let Some(finish_reason) = cand.finish_reason {
                    chunks.push(StreamChunk::Done {
                        finish_reason: Some(finish_reason),
                    });
                }
            }
        }

        if let Some(usage) = self.usage_metadata {
            let prompt = usage.prompt_token_count.unwrap_or(0);
            let completion = usage.candidates_token_count.unwrap_or(0);
            let total = usage.total_token_count.unwrap_or(prompt + completion);
            chunks.push(StreamChunk::Usage(TokenUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: total,
            }));
        }

        chunks
    }
}

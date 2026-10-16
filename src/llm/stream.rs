use std::pin::Pin;
use bytes::Bytes;
use futures::{Stream, StreamExt};

use crate::llm::error::LlmError;
use crate::llm::gemini::response::GeminiStreamResponse;
use crate::llm::types::StreamChunk;

/// Transforma um stream de bytes brutos da resposta HTTP em um stream assíncrono de `StreamChunk`.
pub fn parse_gemini_sse_stream<S>(
    byte_stream: S,
) -> Pin<Box<dyn Stream<Item = Result<StreamChunk, LlmError>> + Send>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let s = async_stream::try_stream! {
        let mut buffer = String::new();
        tokio::pin!(byte_stream);

        while let Some(chunk_res) = byte_stream.next().await {
            let chunk = chunk_res.map_err(LlmError::HttpError)?;
            let text = String::from_utf8_lossy(&chunk);
            buffer.push_str(&text);

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer.drain(..=newline_pos);

                if line.is_empty() {
                    continue;
                }

                if let Some(json_str) = line.strip_prefix("data:") {
                    let trimmed = json_str.trim();
                    if trimmed.is_empty() || trimmed == "[DONE]" {
                        continue;
                    }

                    match serde_json::from_str::<GeminiStreamResponse>(trimmed) {
                        Ok(gemini_resp) => {
                            for chunk in gemini_resp.into_stream_chunks() {
                                yield chunk;
                            }
                        }
                        Err(e) => {
                            yield Err(LlmError::StreamError(format!(
                                "Falha ao decodificar JSON SSE do Gemini: {e}. Payload: {trimmed}"
                            )))?;
                        }
                    }
                }
            }
        }

        // Processa qualquer resto no buffer após o fechamento da conexão
        let remaining = buffer.trim();
        if let Some(json_str) = remaining.strip_prefix("data:") {
            let trimmed = json_str.trim();
            if !trimmed.is_empty()
                && trimmed != "[DONE]"
                && let Ok(gemini_resp) = serde_json::from_str::<GeminiStreamResponse>(trimmed)
            {
                for chunk in gemini_resp.into_stream_chunks() {
                    yield chunk;
                }
            }
        }

    };

    Box::pin(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_parse_gemini_sse_text_chunks() {
        let raw_sse = b"data: {\"candidates\": [{\"content\": {\"parts\": [{\"text\": \"Ol\xc3\xa1 \"}], \"role\": \"model\"}}]}\n\ndata: {\"candidates\": [{\"content\": {\"parts\": [{\"text\": \"Mundo!\"}], \"role\": \"model\"}, \"finishReason\": \"STOP\"}], \"usageMetadata\": {\"promptTokenCount\": 5, \"candidatesTokenCount\": 2, \"totalTokenCount\": 7}}\n\n";

        let byte_stream = stream::iter(vec![Ok(Bytes::from_static(raw_sse))]);
        let mut parsed_stream = parse_gemini_sse_stream(byte_stream);

        let mut collected_text = String::new();
        let mut got_done = false;
        let mut got_usage = false;

        while let Some(res) = parsed_stream.next().await {
            let chunk = res.expect("chunk deve ser Ok");
            match chunk {
                StreamChunk::Text(t) => collected_text.push_str(&t),
                StreamChunk::Done { finish_reason } => {
                    assert_eq!(finish_reason, Some("STOP".to_string()));
                    got_done = true;
                }
                StreamChunk::Usage(u) => {
                    assert_eq!(u.total_tokens, 7);
                    got_usage = true;
                }
                _ => {}
            }
        }

        assert_eq!(collected_text, "Olá Mundo!");
        assert!(got_done);
        assert!(got_usage);
    }
}

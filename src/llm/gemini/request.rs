use serde::{Deserialize, Serialize};

use crate::llm::types::{ChatMessage, ContentPart, Role, ToolDefinition};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiRequest {
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiSystemInstruction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiToolGroup>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeminiContent {
    pub role: String,
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug)]
pub enum GeminiPart {
    Text(String),
    FunctionCall {
        name: String,
        args: serde_json::Value,
    },
    FunctionResponse {
        name: String,
        response: serde_json::Value,
    },
}

// Custom serialization for GeminiPart to match Gemini's { "text": "..." }, { "functionCall": { ... } }, { "functionResponse": { ... } }
impl Serialize for GeminiPart {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            GeminiPart::Text(t) => {
                map.serialize_entry("text", t)?;
            }
            GeminiPart::FunctionCall { name, args } => {
                #[derive(Serialize)]
                struct Call<'a> {
                    name: &'a str,
                    args: &'a serde_json::Value,
                }
                map.serialize_entry("functionCall", &Call { name, args })?;
            }
            GeminiPart::FunctionResponse { name, response } => {
                #[derive(Serialize)]
                struct Resp<'a> {
                    name: &'a str,
                    response: &'a serde_json::Value,
                }
                map.serialize_entry("functionResponse", &Resp { name, response })?;
            }
        }
        map.end()
    }
}

// Custom deserialization for GeminiPart
impl<'de> Deserialize<'de> for GeminiPart {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;
        if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
            return Ok(GeminiPart::Text(text.to_string()));
        }
        if let Some(call) = v.get("functionCall") {
            let name = call.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
            let args = call.get("args").cloned().unwrap_or(serde_json::Value::Null);
            return Ok(GeminiPart::FunctionCall { name, args });
        }
        if let Some(resp) = v.get("functionResponse") {
            let name = resp.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
            let response = resp.get("response").cloned().unwrap_or(serde_json::Value::Null);
            return Ok(GeminiPart::FunctionResponse { name, response });
        }
        Ok(GeminiPart::Text(String::new()))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeminiSystemInstruction {
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiToolGroup {
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeminiFunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

impl GeminiRequest {
    pub fn build(
        system_instruction: Option<&str>,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Self {
        let mut contents = Vec::new();

        for msg in messages {
            let role = match msg.role {
                Role::User => "user".to_string(),
                Role::Assistant => "model".to_string(),
                Role::System => "user".to_string(), // inline fallback if needed
                Role::Tool => "user".to_string(),
            };

            let mut parts = Vec::new();
            for part in &msg.parts {
                match part {
                    ContentPart::Text { text } => {
                        parts.push(GeminiPart::Text(text.clone()));
                    }
                    ContentPart::ToolCall(call) => {
                        parts.push(GeminiPart::FunctionCall {
                            name: call.name.clone(),
                            args: call.arguments.clone(),
                        });
                    }
                    ContentPart::ToolResult(res) => {
                        let response_payload = serde_json::json!({
                            "result": res.content,
                            "is_error": res.is_error
                        });
                        parts.push(GeminiPart::FunctionResponse {
                            name: res.name.clone(),
                            response: response_payload,
                        });
                    }
                }
            }

            if !parts.is_empty() {
                contents.push(GeminiContent { role, parts });
            }
        }

        let system_instruction = system_instruction.map(|sys| GeminiSystemInstruction {
            parts: vec![GeminiPart::Text(sys.to_string())],
        });

        let tools = if tools.is_empty() {
            None
        } else {
            Some(vec![GeminiToolGroup {
                function_declarations: tools
                    .iter()
                    .map(|t| GeminiFunctionDeclaration {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    })
                    .collect(),
            }])
        };

        Self {
            contents,
            system_instruction,
            tools,
            generation_config: Some(GeminiGenerationConfig {
                temperature: Some(0.2),
                max_output_tokens: None,
            }),
        }
    }
}

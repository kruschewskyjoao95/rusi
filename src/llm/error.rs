use thiserror::Error;

#[derive(Error, Debug)]
pub enum LlmError {
    #[error("Chave de API não informada para o provedor '{0}'. Defina a variável de ambiente correspondente ou configure no config.toml")]
    MissingApiKey(String),

    #[error("Erro de requisição HTTP: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Erro retornado pela API do LLM (status {status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("Erro no fluxo SSE de streaming: {0}")]
    StreamError(String),

    #[error("Erro de serialização/deserialização JSON: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Modelo não suportado: '{0}'")]
    UnsupportedModel(String),

    #[error("Erro interno do provedor: {0}")]
    Internal(String),
}

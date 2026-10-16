use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Erros relacionados ao carregamento e salvamento de configurações do Rusi.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Erro de I/O ao acessar arquivo de configuração: {0}")]
    Io(#[from] std::io::Error),

    #[error("Falha ao analisar arquivo TOML de configuração: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("Falha ao serializar configuração para TOML: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("Configuração inválida: {0}")]
    Invalid(String),
}

/// Modo de operação do agente Rusi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum AgentMode {
    /// O agente executa autonomamente até atingir o objetivo ou limite de turnos.
    Auto,
    /// O agente apenas analisa e planeja, sem executar ferramentas de escrita ou destrutivas.
    Plan,
    /// O agente solicita aprovação do usuário para cada ferramenta executada (padrão de segurança).
    #[default]
    AcceptEdits,
}

impl fmt::Display for AgentMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentMode::Auto => write!(f, "auto"),
            AgentMode::Plan => write!(f, "plan"),
            AgentMode::AcceptEdits => write!(f, "accept-edits"),
        }
    }
}

impl std::str::FromStr for AgentMode {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(AgentMode::Auto),
            "plan" => Ok(AgentMode::Plan),
            "accept-edits" | "acceptedits" | "accept_edits" => Ok(AgentMode::AcceptEdits),
            other => Err(ConfigError::Invalid(format!(
                "Modo desconhecido '{other}'. Modos válidos: auto, plan, accept-edits"
            ))),
        }
    }
}

/// Nível de esforço / profundidade de raciocínio (Reasoning Effort).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum EffortLevel {
    Low,
    #[default]
    Medium,
    High,
}

impl fmt::Display for EffortLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EffortLevel::Low => write!(f, "low"),
            EffortLevel::Medium => write!(f, "medium"),
            EffortLevel::High => write!(f, "high"),
        }
    }
}

impl std::str::FromStr for EffortLevel {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Ok(EffortLevel::Low),
            "medium" => Ok(EffortLevel::Medium),
            "high" => Ok(EffortLevel::High),
            other => Err(ConfigError::Invalid(format!(
                "Nível de esforço desconhecido '{other}'. Níveis válidos: low, medium, high"
            ))),
        }
    }
}

/// Configuração de um servidor de protocolo MCP (Model Context Protocol).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Comando ou binário para execução (ex: "uvx", "npx", "docker").
    pub command: String,
    /// Argumentos para o comando.
    #[serde(default)]
    pub args: Vec<String>,
    /// Variáveis de ambiente injetadas no processo do servidor MCP.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Chaves de API para os provedores de LLM suportados.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ApiKeysConfig {
    pub gemini_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
}

impl ApiKeysConfig {
    /// Obtém a chave do Google Gemini com fallback para a variável de ambiente `GEMINI_API_KEY`.
    pub fn gemini_api_key(&self) -> Option<String> {
        self.gemini_api_key
            .clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
    }

    /// Obtém a chave da Anthropic com fallback para a variável de ambiente `ANTHROPIC_API_KEY`.
    pub fn anthropic_api_key(&self) -> Option<String> {
        self.anthropic_api_key
            .clone()
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
    }

    /// Obtém a chave da OpenAI com fallback para a variável de ambiente `OPENAI_API_KEY`.
    pub fn openai_api_key(&self) -> Option<String> {
        self.openai_api_key
            .clone()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
    }
}

/// Configuração global da aplicação Rusi.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Modelo padrão a ser utilizado nas sessões (ex: "gemini-2.0-flash").
    #[serde(default = "default_model")]
    pub default_model: String,

    /// Modo padrão de interação e segurança do agente.
    #[serde(default)]
    pub default_mode: AgentMode,

    /// Nível padrão de raciocínio do modelo.
    #[serde(default)]
    pub default_effort: EffortLevel,

    /// Quando ativo, não solicita confirmação para ações destrutivas ou de escrita.
    #[serde(default)]
    pub dangerously_skip_permissions: bool,

    /// Chaves de API configuradas.
    #[serde(default)]
    pub api_keys: ApiKeysConfig,

    /// Servidores MCP cadastrados.
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

fn default_model() -> String {
    "gemini-2.0-flash".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_model: default_model(),
            default_mode: AgentMode::default(),
            default_effort: EffortLevel::default(),
            dangerously_skip_permissions: false,
            api_keys: ApiKeysConfig::default(),
            mcp_servers: HashMap::new(),
        }
    }
}

impl Config {
    /// Carrega a configuração mesclando:
    /// 1. Arquivo `.env` (se existir) no diretório atual ou ancestrais.
    /// 2. Arquivo de configuração TOML (se explicitado ou nos caminhos padrão).
    /// 3. Variáveis de ambiente com precedência sobre o arquivo TOML.
    pub fn load(custom_path: Option<&Path>) -> Result<Self, ConfigError> {
        // 1. Carrega variáveis de ambiente a partir de .env
        dotenvy::dotenv().ok();

        // 2. Procura pelo arquivo de configuração
        let mut config = if let Some(path) = custom_path {
            Self::from_file(path)?
        } else if let Some(detected_path) = Self::detect_config_path() {
            Self::from_file(&detected_path)?
        } else {
            Config::default()
        };

        // 3. Sobrescreve com variáveis de ambiente quando presentes
        config.apply_env_overrides();

        Ok(config)
    }

    /// Carrega a configuração a partir de um arquivo TOML específico.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::Invalid(format!(
                "Arquivo de configuração não encontrado: {}",
                path.display()
            )));
        }

        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Salva a configuração atual em formato TOML no arquivo especificado.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let serialized = toml::to_string_pretty(self)?;
        std::fs::write(path, serialized)?;
        Ok(())
    }

    /// Detecta a localização de arquivos de configuração respeitando a ordem:
    /// 1. `.rusi/config.toml` no workspace atual.
    /// 2. `rusi.toml` no workspace atual.
    /// 3. Arquivo global do usuário: `~/.config/rusi/config.toml` (Linux/macOS) ou equivalente do sistema.
    pub fn detect_config_path() -> Option<PathBuf> {
        // Workspace local
        let local_workspace_dot = PathBuf::from(".rusi/config.toml");
        if local_workspace_dot.is_file() {
            return Some(local_workspace_dot);
        }

        let local_workspace_file = PathBuf::from("rusi.toml");
        if local_workspace_file.is_file() {
            return Some(local_workspace_file);
        }

        // Global do usuário
        if let Some(global_dir) = Self::global_config_dir() {
            let global_file = global_dir.join("config.toml");
            if global_file.is_file() {
                return Some(global_file);
            }
        }

        None
    }

    /// Retorna o diretório padrão para a configuração global do usuário (`~/.config/rusi`).
    pub fn global_config_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("rusi"))
    }

    /// Aplica sobreposições de variáveis de ambiente conhecidas no sistema.
    fn apply_env_overrides(&mut self) {
        if let Ok(model) = std::env::var("RUSI_MODEL")
            && !model.trim().is_empty()
        {
            self.default_model = model;
        }

        if let Ok(mode_str) = std::env::var("RUSI_MODE")
            && let Ok(mode) = mode_str.parse::<AgentMode>()
        {
            self.default_mode = mode;
        }

        if let Ok(effort_str) = std::env::var("RUSI_EFFORT")
            && let Ok(effort) = effort_str.parse::<EffortLevel>()
        {
            self.default_effort = effort;
        }

        if let Ok(skip) = std::env::var("RUSI_DANGEROUSLY_SKIP_PERMISSIONS")
            && (skip == "1" || skip.eq_ignore_ascii_case("true"))
        {
            self.dangerously_skip_permissions = true;
        }

        if let Ok(gemini_key) = std::env::var("GEMINI_API_KEY")
            && !gemini_key.trim().is_empty()
        {
            self.api_keys.gemini_api_key = Some(gemini_key);
        }

        if let Ok(anthropic_key) = std::env::var("ANTHROPIC_API_KEY")
            && !anthropic_key.trim().is_empty()
        {
            self.api_keys.anthropic_api_key = Some(anthropic_key);
        }

        if let Ok(openai_key) = std::env::var("OPENAI_API_KEY")
            && !openai_key.trim().is_empty()
        {
            self.api_keys.openai_api_key = Some(openai_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_model, "gemini-2.0-flash");
        assert_eq!(config.default_mode, AgentMode::AcceptEdits);
        assert_eq!(config.default_effort, EffortLevel::Medium);
        assert!(!config.dangerously_skip_permissions);
        assert!(config.mcp_servers.is_empty());
    }

    #[test]
    fn test_parse_toml_config() {
        let toml_str = r#"
            default_model = "gemini-2.5-pro"
            default_mode = "plan"
            default_effort = "high"
            dangerously_skip_permissions = true

            [api_keys]
            gemini_api_key = "test-gemini-key"

            [mcp_servers.sqlite]
            command = "uvx"
            args = ["mcp-server-sqlite", "--db-path", "./test.db"]
        "#;

        let config: Config = toml::from_str(toml_str).expect("Falha ao deserializar TOML");
        assert_eq!(config.default_model, "gemini-2.5-pro");
        assert_eq!(config.default_mode, AgentMode::Plan);
        assert_eq!(config.default_effort, EffortLevel::High);
        assert!(config.dangerously_skip_permissions);
        assert_eq!(
            config.api_keys.gemini_api_key.as_deref(),
            Some("test-gemini-key")
        );

        let sqlite_server = config.mcp_servers.get("sqlite").expect("Servidor sqlite não encontrado");
        assert_eq!(sqlite_server.command, "uvx");
        assert_eq!(
            sqlite_server.args,
            vec!["mcp-server-sqlite", "--db-path", "./test.db"]
        );
    }

    #[test]
    fn test_mode_from_str() {
        assert_eq!("auto".parse::<AgentMode>().unwrap(), AgentMode::Auto);
        assert_eq!("plan".parse::<AgentMode>().unwrap(), AgentMode::Plan);
        assert_eq!("accept-edits".parse::<AgentMode>().unwrap(), AgentMode::AcceptEdits);
        assert!("invalid".parse::<AgentMode>().is_err());
    }

    #[test]
    fn test_effort_from_str() {
        assert_eq!("low".parse::<EffortLevel>().unwrap(), EffortLevel::Low);
        assert_eq!("medium".parse::<EffortLevel>().unwrap(), EffortLevel::Medium);
        assert_eq!("high".parse::<EffortLevel>().unwrap(), EffortLevel::High);
        assert!("ultra".parse::<EffortLevel>().is_err());
    }
}

use clap::{Args, Parser, Subcommand};
use colored::Colorize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::{AgentMode, Config, EffortLevel, McpServerConfig};

/// Rusi — Orquestrador autônomo de agentes de IA em Rust 🦀
#[derive(Parser, Debug)]
#[command(
    name = "rusi",
    author,
    version,
    about = "Orquestrador autônomo de agentes de IA em Rust 🦀",
    long_about = "Rusi é um orquestrador de agentes de linha de comando autônomo, seguro e de altíssima performance, construído em Rust."
)]
pub struct Cli {
    /// Prompt inicial ou instrução direta para o agente executar
    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,

    /// Caminho customizado para o arquivo de configuração TOML
    #[arg(short = 'C', long = "config", value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Modelo LLM a ser utilizado (ex: gemini-2.0-flash, gemini-2.5-pro, claude-3-7-sonnet)
    #[arg(short = 'm', long = "model", env = "RUSI_MODEL")]
    pub model: Option<String>,

    /// Modo de execução e permissões do agente
    #[arg(long = "mode", value_enum)]
    pub mode: Option<AgentMode>,

    /// Nível de esforço e raciocínio (reasoning effort)
    #[arg(long = "effort", value_enum)]
    pub effort: Option<EffortLevel>,

    /// Executa ferramentas de escrita ou comandos do sistema sem solicitar confirmação manual
    #[arg(long = "dangerously-skip-permissions")]
    pub dangerously_skip_permissions: bool,

    /// Executa em modo não-interativo, apenas imprimindo a resposta final
    #[arg(short = 'p', long = "print")]
    pub print: bool,

    /// Continua o contexto e histórico da sessão anterior
    #[arg(short = 'c', long = "continue")]
    pub r#continue: bool,

    /// Subcomandos de gerenciamento e utilitários
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Gerenciar servidores MCP (Model Context Protocol)
    Mcp(McpArgs),

    /// Listar modelos e status dos provedores de IA suportados
    Models,

    /// Gerenciar ou inspecionar as configurações do Rusi
    Config(ConfigArgs),

    /// Exibir informações de instalação e autocomplete
    Install,

    /// Verificar versão e atualizações do Rusi
    Update,
}

#[derive(Args, Debug)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommands,
}

#[derive(Subcommand, Debug)]
pub enum McpCommands {
    /// Adicionar um novo servidor MCP
    Add {
        /// Identificador único para o servidor MCP
        name: String,
        /// Comando ou binário para execução (ex: uvx, npx, docker)
        command: String,
        /// Argumentos adicionais repassados ao comando
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Remover um servidor MCP cadastrado
    #[command(alias = "rm")]
    Remove {
        /// Nome do servidor MCP a ser removido
        name: String,
    },
    /// Listar todos os servidores MCP configurados
    #[command(alias = "ls")]
    List,
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: Option<ConfigCommands>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Exibir o caminho do arquivo de configuração ativo
    Path,
    /// Exibir a configuração completa ativa em formato TOML
    Show,
    /// Obter o valor de uma chave específica
    Get {
        /// Nome da chave (ex: default_model, default_mode, default_effort)
        key: String,
    },
    /// Definir ou atualizar o valor de uma chave na configuração
    Set {
        /// Nome da chave
        key: String,
        /// Novo valor para a chave
        value: String,
    },
}

impl Cli {
    /// Mescla os argumentos da linha de comando com a configuração carregada,
    /// garantindo que as flags de linha de comando tenham a maior precedência.
    pub fn merge_with_config(&self, mut config: Config) -> Config {
        if let Some(ref model) = self.model {
            config.default_model = model.clone();
        }

        if let Some(mode) = self.mode {
            config.default_mode = mode;
        }

        if let Some(effort) = self.effort {
            config.default_effort = effort;
        }

        if self.dangerously_skip_permissions {
            config.dangerously_skip_permissions = true;
        }

        config
    }
}

/// Manipulador de comandos utilitários da CLI.
pub fn handle_command(cmd: &Commands, config: &mut Config) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        Commands::Mcp(mcp_args) => handle_mcp_command(&mcp_args.command, config)?,
        Commands::Models => handle_models_command(config),
        Commands::Config(cfg_args) => handle_config_command(cfg_args.command.as_ref(), config)?,
        Commands::Install => handle_install_command(),
        Commands::Update => handle_update_command(),
    }
    Ok(())
}

fn handle_mcp_command(
    cmd: &McpCommands,
    config: &mut Config,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        McpCommands::Add { name, command, args } => {
            println!(
                "{} Adicionando servidor MCP '{}'...",
                "🦀".bold(),
                name.cyan().bold()
            );

            config.mcp_servers.insert(
                name.clone(),
                McpServerConfig {
                    command: command.clone(),
                    args: args.clone(),
                    env: HashMap::new(),
                },
            );

            let target_path = Config::detect_config_path()
                .or_else(|| Config::global_config_dir().map(|d| d.join("config.toml")))
                .unwrap_or_else(|| PathBuf::from("rusi.toml"));

            config.save_to(&target_path)?;
            println!(
                "{} Servidor MCP '{}' salvo com sucesso em {}!",
                "✔".green().bold(),
                name.cyan(),
                target_path.display().to_string().yellow()
            );
        }
        McpCommands::Remove { name } => {
            if config.mcp_servers.remove(name).is_some() {
                let target_path = Config::detect_config_path()
                    .or_else(|| Config::global_config_dir().map(|d| d.join("config.toml")))
                    .unwrap_or_else(|| PathBuf::from("rusi.toml"));

                config.save_to(&target_path)?;
                println!(
                    "{} Servidor MCP '{}' removido com sucesso.",
                    "✔".green().bold(),
                    name.yellow()
                );
            } else {
                eprintln!(
                    "{} Servidor MCP '{}' não encontrado.",
                    "✖".red().bold(),
                    name
                );
            }
        }
        McpCommands::List => {
            println!("\n{}", "=== Servidores MCP Configurados ===".cyan().bold());
            if config.mcp_servers.is_empty() {
                println!(
                    "{}",
                    "Nenhum servidor MCP cadastrado. Adicione um com: rusi mcp add <nome> <comando> [args...]"
                        .dimmed()
                );
            } else {
                for (name, server) in &config.mcp_servers {
                    println!(
                        "• {} {} {}",
                        name.green().bold(),
                        "->".dimmed(),
                        server.command.bold()
                    );
                    if !server.args.is_empty() {
                        println!("  Argumentos: {}", server.args.join(" ").dimmed());
                    }
                }
            }
            println!();
        }
    }
    Ok(())
}

fn handle_models_command(config: &Config) {
    println!("\n{}", "=== Provedores & Modelos Suportados ===".cyan().bold());

    // Provedor Google Gemini
    let gemini_status = if config.api_keys.gemini_api_key().is_some() {
        "Configurada ✔".green().bold()
    } else {
        "Não configurada (defina GEMINI_API_KEY)".yellow()
    };
    println!(
        "\n{} {}",
        "Google Gemini".bold().underline(),
        format!("[API Key: {gemini_status}]").dimmed()
    );
    println!("  • gemini-2.5-pro       {}", "(Reasoning avançado, código complexo)".dimmed());
    println!("  • gemini-2.0-flash     {}", "(Altíssima velocidade, padrão recomendado)".dimmed());
    println!("  • gemini-2.0-flash-lite{}", "(Econômico, respostas imediatas)".dimmed());

    // Provedor Anthropic Claude
    let claude_status = if config.api_keys.anthropic_api_key().is_some() {
        "Configurada ✔".green().bold()
    } else {
        "Não configurada (defina ANTHROPIC_API_KEY)".yellow()
    };
    println!(
        "\n{} {}",
        "Anthropic Claude".bold().underline(),
        format!("[API Key: {claude_status}]").dimmed()
    );
    println!("  • claude-3-7-sonnet    {}", "(Hybrid reasoning, engenharia de ponta)".dimmed());
    println!("  • claude-3-5-haiku     {}", "(Rápido e conciso)".dimmed());

    // Provedor OpenAI
    let openai_status = if config.api_keys.openai_api_key().is_some() {
        "Configurada ✔".green().bold()
    } else {
        "Não configurada (defina OPENAI_API_KEY)".yellow()
    };
    println!(
        "\n{} {}",
        "OpenAI".bold().underline(),
        format!("[API Key: {openai_status}]").dimmed()
    );
    println!("  • gpt-4o               {}", "(Multimodal, raciocínio robusto)".dimmed());
    println!("  • o3-mini              {}", "(Raciocínio lógico e matemática)".dimmed());

    // Modelos Locais (Ollama)
    println!("\n{}", "Modelos Locais (Ollama / Compatíveis)".bold().underline());
    println!("  • ollama/qwen2.5-coder {}", "(Open-source líder em código)".dimmed());
    println!("  • ollama/deepseek-r1   {}", "(Raciocínio local)".dimmed());

    println!("\n{} {}", "Modelo atual ativo:".bold(), config.default_model.cyan().bold());
    println!();
}

fn handle_config_command(
    cmd: Option<&ConfigCommands>,
    config: &mut Config,
) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        Some(ConfigCommands::Path) => {
            if let Some(path) = Config::detect_config_path() {
                println!(
                    "{} Arquivo de configuração ativo: {}",
                    "✔".green().bold(),
                    path.display().to_string().cyan().bold()
                );
            } else {
                let default_global = Config::global_config_dir()
                    .map(|d| d.join("config.toml"))
                    .unwrap_or_else(|| PathBuf::from("config.toml"));
                println!(
                    "{} Nenhum arquivo encontrado. O caminho global padrão é: {}",
                    "ℹ".blue().bold(),
                    default_global.display().to_string().yellow()
                );
            }
        }
        Some(ConfigCommands::Show) => {
            println!("\n{}", "=== Configuração Atual Ativa ===".cyan().bold());
            let toml_str = toml::to_string_pretty(config)?;
            println!("{toml_str}");
        }
        Some(ConfigCommands::Get { key }) => match key.as_str() {
            "default_model" | "model" => println!("{}", config.default_model),
            "default_mode" | "mode" => println!("{}", config.default_mode),
            "default_effort" | "effort" => println!("{}", config.default_effort),
            "dangerously_skip_permissions" => println!("{}", config.dangerously_skip_permissions),
            other => eprintln!("{} Chave desconhecida: '{other}'", "✖".red().bold()),
        },
        Some(ConfigCommands::Set { key, value }) => {
            match key.as_str() {
                "default_model" | "model" => config.default_model = value.clone(),
                "default_mode" | "mode" => config.default_mode = value.parse()?,
                "default_effort" | "effort" => config.default_effort = value.parse()?,
                "dangerously_skip_permissions" => {
                    config.dangerously_skip_permissions = value == "true" || value == "1";
                }
                other => {
                    eprintln!("{} Chave desconhecida: '{other}'", "✖".red().bold());
                    return Ok(());
                }
            }

            let target_path = Config::detect_config_path()
                .or_else(|| Config::global_config_dir().map(|d| d.join("config.toml")))
                .unwrap_or_else(|| PathBuf::from("rusi.toml"));

            config.save_to(&target_path)?;
            println!(
                "{} Chave '{}' atualizada para '{}' em {}",
                "✔".green().bold(),
                key.cyan(),
                value.green(),
                target_path.display().to_string().yellow()
            );
        }
        None => {
            println!(
                "{} Use '{}' para ver os subcomandos disponíveis.",
                "ℹ".blue().bold(),
                "rusi config --help".yellow()
            );
        }
    }
    Ok(())
}

fn handle_install_command() {
    println!("\n{}", "=== Instalação do Rusi 🦀 ===".cyan().bold());
    println!("Para instalar o binário no seu PATH global via Cargo, execute:");
    println!("  {}", "cargo install --path .".yellow().bold());
    println!("\nPara gerar autocompletion para seu shell (bash/zsh/fish), consulte a documentação.");
    println!();
}

fn handle_update_command() {
    println!("\n{}", "=== Atualização do Rusi 🦀 ===".cyan().bold());
    println!("Versão atual: {}", env!("CARGO_PKG_VERSION").green().bold());
    println!("Você está executando a versão de desenvolvimento local.");
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_prompt_argument() {
        let args = ["rusi", "analise o projeto"];
        let cli = Cli::try_parse_from(args).expect("Falha ao analisar argumentos");
        assert_eq!(cli.prompt.as_deref(), Some("analise o projeto"));
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_parse_flags() {
        let args = [
            "rusi",
            "--model",
            "gemini-2.5-pro",
            "--mode",
            "plan",
            "--effort",
            "high",
            "--dangerously-skip-permissions",
            "-p",
            "-c",
        ];
        let cli = Cli::try_parse_from(args).expect("Falha ao analisar flags");
        assert_eq!(cli.model.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(cli.mode, Some(AgentMode::Plan));
        assert_eq!(cli.effort, Some(EffortLevel::High));
        assert!(cli.dangerously_skip_permissions);
        assert!(cli.print);
        assert!(cli.r#continue);
    }

    #[test]
    fn test_parse_mcp_add_subcommand() {
        let args = [
            "rusi",
            "mcp",
            "add",
            "sqlite",
            "uvx",
            "mcp-server-sqlite",
            "--db-path",
            "./test.db",
        ];
        let cli = Cli::try_parse_from(args).expect("Falha ao analisar subcomando mcp add");
        match cli.command {
            Some(Commands::Mcp(McpArgs {
                command: McpCommands::Add { name, command, args },
            })) => {
                assert_eq!(name, "sqlite");
                assert_eq!(command, "uvx");
                assert_eq!(args, vec!["mcp-server-sqlite", "--db-path", "./test.db"]);
            }
            _ => panic!("Esperado subcomando McpCommands::Add"),
        }
    }

    #[test]
    fn test_cli_merge_with_config() {
        let args = ["rusi", "--model", "custom-model", "--mode", "auto"];
        let cli = Cli::try_parse_from(args).expect("Falha ao analisar args");
        let base_config = Config::default();

        let merged = cli.merge_with_config(base_config);
        assert_eq!(merged.default_model, "custom-model");
        assert_eq!(merged.default_mode, AgentMode::Auto);
    }
}


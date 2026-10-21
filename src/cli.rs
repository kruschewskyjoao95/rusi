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

    /// Provedor de agente a hospedar (ex: claude, codex, antigravity)
    #[arg(long = "provider", env = "RUSI_PROVIDER")]
    pub provider: Option<String>,

    /// Modelo a pedir ao provedor (padrão: o do próprio provedor)
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

    /// Listar provedores detectados, versão e estado de login
    Providers,

    /// Gerenciar ou inspecionar as configurações do Rusi
    Config(ConfigArgs),

    /// Listar e gerenciar as sessões (threads) salvas
    Threads(ThreadsArgs),

    /// Guardar segredos de provedores no keyring do sistema
    Secrets(SecretsArgs),

    /// Worktrees Git por tarefa
    Worktree(WorktreeArgs),

    /// Status e diff do checkout atual
    Git(GitArgs),

    /// Mostrar os arquivos alterados em cada turno de uma sessão
    Changes {
        /// Id da sessão
        thread: String,
    },

    /// Exibir informações de instalação e autocomplete
    Install,

    /// Verificar versão e atualizações do Rusi
    Update,
}

#[derive(Args, Debug)]
pub struct WorktreeArgs {
    #[command(subcommand)]
    pub command: WorktreeCommands,
}

#[derive(Subcommand, Debug)]
pub enum WorktreeCommands {
    /// Cria uma branch num worktree novo (`<projeto>-worktrees/<branch>`)
    New {
        branch: String,
        /// Branch/commit de partida (padrão: HEAD)
        #[arg(long)]
        base: Option<String>,
        /// Sessão que passa a trabalhar nesse worktree
        #[arg(long)]
        thread: Option<String>,
    },
    /// Lista os worktrees do projeto
    List,
    /// Remove um worktree (recusa se houver alterações não commitadas)
    Rm { path: PathBuf },
}

#[derive(Args, Debug)]
pub struct GitArgs {
    #[command(subcommand)]
    pub command: GitCommands,
}

#[derive(Subcommand, Debug)]
pub enum GitCommands {
    /// Arquivos alterados
    Status,
    /// Diff unificado
    Diff {
        /// Compara o que está no índice com HEAD
        #[arg(long)]
        staged: bool,
    },
}

#[derive(Args, Debug)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommands,
}

#[derive(Args, Debug)]
pub struct ThreadsArgs {
    #[command(subcommand)]
    pub command: ThreadsCommands,
}

#[derive(Subcommand, Debug)]
pub enum ThreadsCommands {
    /// Listar sessões (fixadas primeiro, depois as mais recentes)
    List {
        /// Número máximo de sessões
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Fixar uma sessão no topo da lista
    Pin { id: String },
    /// Desafixar uma sessão
    Unpin { id: String },
    /// Remover uma sessão e seus eventos
    Delete { id: String },
}

#[derive(Args, Debug)]
pub struct SecretsArgs {
    #[command(subcommand)]
    pub command: SecretsCommands,
}

#[derive(Subcommand, Debug)]
pub enum SecretsCommands {
    /// Gravar um segredo; o valor é lido da entrada padrão (nunca de argumentos)
    Set { provider: String, key: String },
    /// Informar se um segredo existe, sem exibir o valor
    Status { provider: String, key: String },
    /// Apagar um segredo
    Delete { provider: String, key: String },
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
        if let Some(ref provider) = self.provider {
            config.default_provider = provider.clone();
        }

        if let Some(ref model) = self.model {
            config.default_model = Some(model.clone());
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
        Commands::Providers => handle_providers_command(config),
        Commands::Threads(args) => handle_threads_command(&args.command)?,
        Commands::Secrets(args) => handle_secrets_command(&args.command)?,
        Commands::Worktree(args) => block_on(handle_worktree_command(&args.command))?,
        Commands::Git(args) => block_on(handle_git_command(&args.command))?,
        Commands::Changes { thread } => handle_changes_command(thread)?,
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

fn handle_providers_command(config: &Config) {
    use crate::provider::{AuthState, built_in_drivers, snapshot_all};

    println!("\n{}", "=== Provedores ===".cyan().bold());
    let drivers = built_in_drivers();
    let snapshots = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(snapshot_all(&drivers))
    });

    for snap in snapshots {
        let auth = match &snap.auth {
            AuthState::Authenticated { account: Some(a) } => format!("logado ({a})").green().bold(),
            AuthState::Authenticated { account: None } => "logado".green().bold(),
            AuthState::Unauthenticated => "sem login".yellow().bold(),
            AuthState::Unknown => "login não verificado".dimmed(),
            AuthState::Unavailable => "não instalado".red(),
        };
        let active = if snap.id == config.default_provider { " ◀ ativo".cyan().bold() } else { "".normal() };
        println!(
            "\n{} {}{}",
            snap.display_name.bold().underline(),
            format!("[{auth}]").dimmed(),
            active
        );
        if let Some(version) = &snap.version {
            println!("  versão: {version}");
        }
        if let Some(binary) = &snap.binary {
            println!("  binário: {}", binary.display().to_string().dimmed());
        }
        if let Some(message) = &snap.message {
            println!("  {}", message.yellow());
        }
    }
    println!();
}

/// Abre o banco padrão do Rusi.
pub fn open_database() -> Result<crate::persistence::Database, Box<dyn std::error::Error>> {
    let path = crate::persistence::Database::default_path()
        .ok_or("não foi possível determinar o diretório de dados do usuário")?;
    Ok(crate::persistence::Database::open(&path)?)
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

async fn handle_worktree_command(cmd: &WorktreeCommands) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    match cmd {
        WorktreeCommands::New { branch, base, thread } => {
            let path = match thread {
                Some(id) => {
                    let db = open_database()?;
                    db.attach_worktree(id, &cwd, branch).await?
                }
                None => crate::git::create_worktree(&cwd, branch, base.as_deref()).await?,
            };
            println!("{} {}", "✔".green().bold(), path.display().to_string().cyan());
        }
        WorktreeCommands::List => print!("{}", crate::git::worktree_list(&cwd).await?),
        WorktreeCommands::Rm { path } => {
            crate::git::remove_worktree(&cwd, path).await?;
            println!("{} Worktree removido.", "✔".green().bold());
        }
    }
    Ok(())
}

async fn handle_git_command(cmd: &GitCommands) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    match cmd {
        GitCommands::Status => {
            let files = crate::git::status(&cwd).await?;
            if files.is_empty() {
                println!("{} Árvore limpa.", "ℹ".blue().bold());
            }
            for f in files {
                println!("{}{} {}", f.index, f.worktree, f.path);
            }
        }
        GitCommands::Diff { staged } => print!("{}", crate::git::diff(&cwd, *staged).await?),
    }
    Ok(())
}

fn handle_changes_command(thread: &str) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_database()?;
    db.get_thread(thread)?;
    let mut turn = 0;
    for event in db.thread_events(thread, 0)? {
        match event.kind.as_str() {
            "user" => turn += 1,
            "changes" => {
                println!("{}", format!("Turno {turn}").bold());
                for f in event.payload["files"].as_array().into_iter().flatten() {
                    let n = |k: &str| f[k].as_u64().map_or("-".into(), |v| v.to_string());
                    println!("  {} {} {}", format!("+{}", n("added")).green(), format!("-{}", n("deleted")).red(), f["path"].as_str().unwrap_or("?"));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn handle_threads_command(cmd: &ThreadsCommands) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_database()?;
    match cmd {
        ThreadsCommands::List { limit } => {
            let threads = db.list_threads(*limit)?;
            if threads.is_empty() {
                println!("{} Nenhuma sessão salva.", "ℹ".blue().bold());
            }
            for t in threads {
                println!(
                    "{} {} {} {}",
                    if t.pinned { "📌" } else { "  " },
                    t.id.cyan(),
                    t.provider.bold(),
                    t.title.as_deref().unwrap_or("(sem título)").dimmed()
                );
            }
        }
        ThreadsCommands::Pin { id } => {
            db.set_thread_pinned(id, true)?;
            println!("{} Sessão fixada.", "✔".green().bold());
        }
        ThreadsCommands::Unpin { id } => {
            db.set_thread_pinned(id, false)?;
            println!("{} Sessão desafixada.", "✔".green().bold());
        }
        ThreadsCommands::Delete { id } => {
            if db.delete_thread(id)? {
                println!("{} Sessão removida.", "✔".green().bold());
            } else {
                eprintln!("{} Sessão não encontrada: {id}", "✖".red().bold());
            }
        }
    }
    Ok(())
}

fn handle_secrets_command(cmd: &SecretsCommands) -> Result<(), Box<dyn std::error::Error>> {
    use crate::secrets::{KeyringStore, SecretStore};
    let store = KeyringStore::new();
    match cmd {
        SecretsCommands::Set { provider, key } => {
            let mut value = String::new();
            std::io::stdin().read_line(&mut value)?;
            let value = value.trim_end_matches(['\r', '\n']);
            if value.is_empty() {
                return Err("valor vazio: envie o segredo pela entrada padrão".into());
            }
            store.set(provider, key, value)?;
            println!("{} Segredo gravado para {provider}/{key}.", "✔".green().bold());
        }
        SecretsCommands::Status { provider, key } => {
            let state = if store.get(provider, key)?.is_some() { "presente".green() } else { "ausente".yellow() };
            println!("{provider}/{key}: {state}");
        }
        SecretsCommands::Delete { provider, key } => {
            if store.delete(provider, key)? {
                println!("{} Segredo removido.", "✔".green().bold());
            } else {
                println!("{} Nenhum segredo para {provider}/{key}.", "ℹ".blue().bold());
            }
        }
    }
    Ok(())
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
            "default_provider" | "provider" => println!("{}", config.default_provider),
            "default_model" | "model" => println!("{}", config.default_model.as_deref().unwrap_or("")),
            "default_mode" | "mode" => println!("{}", config.default_mode),
            "default_effort" | "effort" => println!("{}", config.default_effort),
            "dangerously_skip_permissions" => println!("{}", config.dangerously_skip_permissions),
            other => eprintln!("{} Chave desconhecida: '{other}'", "✖".red().bold()),
        },
        Some(ConfigCommands::Set { key, value }) => {
            match key.as_str() {
                "default_provider" | "provider" => config.default_provider = value.clone(),
                "default_model" | "model" => config.default_model = Some(value.clone()),
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
            "custom-model",
            "--provider",
            "codex",
            "--mode",
            "plan",
            "--effort",
            "high",
            "--dangerously-skip-permissions",
            "-p",
            "-c",
        ];
        let cli = Cli::try_parse_from(args).expect("Falha ao analisar flags");
        assert_eq!(cli.model.as_deref(), Some("custom-model"));
        assert_eq!(cli.provider.as_deref(), Some("codex"));
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
        assert_eq!(merged.default_model.as_deref(), Some("custom-model"));
        assert_eq!(merged.default_mode, AgentMode::Auto);
    }
}


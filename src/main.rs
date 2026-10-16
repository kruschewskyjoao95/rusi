use rusi::cli;
use rusi::config;
use rusi::llm;
pub use rusi::tools;

use std::io::{self, Write};
use clap::Parser;
use colored::Colorize;
use tokio_stream::StreamExt;

use cli::Cli;
use config::Config;
use llm::{ChatMessage, StreamChunk};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // 1. Carrega configuração a partir de arquivo e ambiente
    let mut config = Config::load(cli.config.as_deref())?;

    // 2. Se um subcomando foi informado, delega e encerra
    if let Some(ref cmd) = cli.command {
        cli::handle_command(cmd, &mut config)?;
        return Ok(());
    }

    // 3. Mescla flags da CLI sobrepondo as configurações carregadas
    let config = cli.merge_with_config(config);

    // 4. Banner principal do Rusi
    println!(
        "{} {} {}",
        "🦀".bold(),
        "Rusi".cyan().bold(),
        format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
    );
    println!(
        "{} {}  {} {}  {} {}",
        "Modelo:".bold(),
        config.default_model.cyan(),
        "Modo:".bold(),
        config.default_mode.to_string().yellow(),
        "Esforço:".bold(),
        config.default_effort.to_string().magenta()
    );

    if config.dangerously_skip_permissions {
        println!(
            "{} {}",
            "Aviso:".red().bold(),
            "Permissões ignoradas (--dangerously-skip-permissions ativo)!".yellow()
        );
    }

    // 5. Verifica se um prompt imediato foi fornecido
    if let Some(prompt) = cli.prompt {
        println!("\n{} {}", "Instrução:".bold(), prompt.green());

        match llm::create_provider(&config.default_model, &config) {
            Ok(provider) => {
                println!(
                    "{} {} ({})",
                    "⚡ Conectando ao provedor:".dimmed(),
                    provider.provider_name().bold(),
                    provider.model_name().cyan()
                );

                let messages = vec![ChatMessage::user(&prompt)];
                match provider.stream_completion(None, &messages, &[]).await {
                    Ok(mut stream) => {
                        println!();
                        let mut token_usage = None;

                        while let Some(chunk_res) = stream.next().await {
                            match chunk_res {
                                Ok(StreamChunk::Text(text)) => {
                                    print!("{text}");
                                    let _ = io::stdout().flush();
                                }
                                Ok(StreamChunk::FunctionCall(call)) => {
                                    println!(
                                        "\n{} {}({})",
                                        "🔧 [Chamada de Ferramenta]:".yellow().bold(),
                                        call.name.bold(),
                                        call.arguments
                                    );
                                }
                                Ok(StreamChunk::Usage(usage)) => {
                                    token_usage = Some(usage);
                                }
                                Ok(StreamChunk::Done { finish_reason }) => {
                                    if let Some(reason) = finish_reason
                                        && reason != "STOP"
                                    {
                                        println!("\n{} {}", "Fim da geração:".dimmed(), reason.yellow());
                                    }
                                }

                                Err(err) => {
                                    eprintln!("\n{} {}", "Erro no streaming:".red().bold(), err);
                                    break;
                                }
                            }
                        }
                        println!();

                        if let Some(usage) = token_usage {
                            println!(
                                "\n{} {} (Prompt: {}, Saída: {})",
                                "📊 Tokens utilizados:".dimmed(),
                                usage.total_tokens.to_string().bold(),
                                usage.prompt_tokens,
                                usage.completion_tokens
                            );
                        }
                    }
                    Err(err) => {
                        eprintln!("\n{} {}", "Erro ao iniciar stream:".red().bold(), err);
                    }
                }
            }
            Err(err) => {
                eprintln!("\n{} {}", "Erro de configuração do modelo:".red().bold(), err);
                println!(
                    "{}",
                    "Dica: certifique-se de configurar GEMINI_API_KEY no seu ambiente ou via .env".yellow()
                );
            }
        }
    } else {
        println!(
            "\n{}",
            "Sessão interativa pronta. Digite seus comandos ou use --help para ver as opções."
                .dimmed()
        );
    }

    Ok(())
}


use rusi::chat;
use rusi::cli;
use rusi::config;

use clap::Parser;
use colored::Colorize;

use cli::Cli;
use config::Config;

#[tokio::main(flavor = "multi_thread")]
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

    if !cli.print {
        println!(
            "{} {} {}",
            "🦀".bold(),
            "Rusi".cyan().bold(),
            format!("v{}", env!("CARGO_PKG_VERSION")).dimmed()
        );
    }

    let options = chat::ChatOptions { prompt: cli.prompt, print_only: cli.print, resume_latest: cli.r#continue };
    if let Err(err) = chat::run(&config, options).await {
        eprintln!("{} {err}", "Erro:".red().bold());
        std::process::exit(1);
    }
    Ok(())
}

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use rss_ai::config::Config;

#[derive(Parser)]
#[command(name = "rss-ai", about = "AI-powered RSS reader")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the RSS-AI service
    Serve {
        /// Path to a TOML config file (uses default location if omitted)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Show or generate the TOML configuration
    Config {
        /// Write the commented default config to the default config path
        #[arg(long)]
        generate: bool,
        /// Load and display the resolved config from this file
        #[arg(long)]
        file: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Serve { config: path } => {
            let cfg = Config::load(path.as_deref())?;
            println!("RSS-AI service starting...");
            println!("  data_dir : {}", cfg.data_dir().display());
            println!("  log_level: {}", cfg.service.log_level);
        }
        Command::Config { generate, file } => {
            if generate {
                let path = Config::default_path();
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, Config::default_toml_with_comments())?;
                println!("Config written to {}", path.display());
            } else {
                let cfg = Config::load(file.as_deref())?;
                let toml_str = toml::to_string_pretty(&cfg)?;
                print!("{toml_str}");
            }
        }
    }

    Ok(())
}

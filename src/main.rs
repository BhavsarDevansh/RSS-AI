use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rss-ai", about = "AI-powered RSS reader")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the RSS-AI service
    Serve,
    /// Print a default TOML config template to stdout
    Config,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Serve => {
            println!("RSS-AI service starting...");
        }
        Command::Config => {
            print!(
                r#"[feeds]
urls = [
    "https://example.com/feed.xml",
]

[database]
path = "data/rss_ai.db"

[search]
index_path = "data/index"

[embeddings]
model = "default"

[server]
host = "127.0.0.1"
port = 8080
"#
            );
        }
    }
}

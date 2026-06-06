use anyhow::Result;
use clap::{Parser, Subcommand};
use crossbeam_channel::bounded;
use shrimp_core::{Agent, AgentEvent, ShrimpConfig};
use shrimp_provider::ProviderKind;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "shrimp", about = "Local coding agent", version)]
struct Cli {
    #[arg(long, default_value = ".", env = "SHRIMP_REPO")]
    repo: PathBuf,

    #[arg(long, env = "SHRIMP_PROVIDER")]
    provider: Option<String>,

    #[arg(long, env = "SHRIMP_MODEL")]
    model: Option<String>,

    #[arg(long, env = "SHRIMP_BASE_URL")]
    base_url: Option<String>,

    #[arg(short = 'P', long)]
    prompt: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Index {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    Status,
    Run {
        #[arg(short = 'P', long)]
        prompt: String,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

fn parse_provider(s: &str) -> ProviderKind {
    match s.to_lowercase().replace('-', "_").as_str() {
        "lmstudio" | "lm_studio" => ProviderKind::LmStudio,
        _ => ProviderKind::Ollama,
    }
}

fn apply_overrides(cfg: &mut ShrimpConfig, cli: &Cli) {
    let old_provider = cfg.provider.clone();
    if let Some(p) = &cli.provider {
        cfg.provider = parse_provider(p);
    }
    if let Some(m) = &cli.model {
        cfg.model = m.clone();
    }
    if let Some(u) = &cli.base_url {
        cfg.base_url = cfg.provider.resolve_base_url(Some(u));
    } else if cfg.provider != old_provider {
        cfg.base_url = cfg.provider.resolve_base_url(None);
    }
}

fn run_headless(prompt: &str, cfg: ShrimpConfig) -> Result<()> {
    let (tx, rx) = bounded::<AgentEvent>(256);
    let mut agent = Agent::new(cfg, tx)?;

    std::thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            match ev {
                AgentEvent::ToolStart { name, args } => {
                    eprintln!("  ⚙ {} {}", name, args);
                }
                AgentEvent::ToolEnd {
                    name,
                    success,
                    output,
                    ..
                } => {
                    let mark = if success { "✓" } else { "✗" };
                    let snip: String = output.chars().take(120).collect();
                    eprintln!("  {} {} → {}", mark, name, snip);
                }
                AgentEvent::IndexReady {
                    symbols,
                    files,
                    duration_ms,
                } => {
                    eprintln!(
                        "  indexed {} symbols in {} files ({}ms)",
                        symbols, files, duration_ms
                    );
                }
                AgentEvent::Error { message } => {
                    eprintln!("  error: {}", message);
                }
                _ => {}
            }
        }
    });

    let response = agent.run_turn(prompt)?;
    println!("{}", response);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Index { repo }) => {
            let abs = repo.canonicalize().unwrap_or(repo);
            let mut cfg = ShrimpConfig::load(&abs);
            cfg.repo_root = abs.clone();
            let index_dir = cfg.index_dir();
            std::fs::create_dir_all(&index_dir)?;
            let stats = shrimp_index::build_index(&abs, &index_dir)?;
            println!(
                "indexed {} files ({} changed, {} removed) · {} symbols · {}ms",
                stats.files_total,
                stats.files_changed,
                stats.files_removed,
                stats.symbols_total,
                stats.duration_ms
            );
        }

        Some(Commands::Status) => {
            let abs = cli.repo.canonicalize().unwrap_or(cli.repo.clone());
            let mut cfg = ShrimpConfig::load(&abs);
            apply_overrides(&mut cfg, &cli);
            println!("shrimp v{}", env!("CARGO_PKG_VERSION"));
            println!("repo     : {}", cfg.repo_root.display());
            println!("provider : {}", cfg.provider.as_str());
            println!("model    : {}", cfg.model);
            println!("base_url : {}", cfg.base_url);
        }

        Some(Commands::Run { ref prompt, ref repo }) => {
            let abs = repo.canonicalize().unwrap_or(repo.clone());
            let mut cfg = ShrimpConfig::load(&abs);
            cfg.repo_root = abs;
            cfg.auto_approve = true;
            apply_overrides(&mut cfg, &cli);
            run_headless(prompt, cfg)?;
        }


        None => {
            let abs = cli.repo.canonicalize().unwrap_or(cli.repo.clone());
            let mut cfg = ShrimpConfig::load(&abs);
            cfg.repo_root = abs;
            apply_overrides(&mut cfg, &cli);

            if let Some(prompt) = cli.prompt {
                run_headless(&prompt, cfg)?;
            } else {
                shrimp_tui::run_app(cfg).await?;
            }
        }
    }

    Ok(())
}

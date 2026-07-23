mod agent;
mod cli;
mod llm;
mod tools;

use std::env;
use std::path::PathBuf;

use clap::Parser;
use tokio::sync::mpsc;

use agent::{AgentConfig, AgentEvent, AgentLoop};
use tools::{
    bash::BashTool, glob::GlobTool, grep::GrepTool, read::ReadTool, registry::ToolRegistry,
    write::WriteTool,
};

#[derive(Parser)]
#[command(name = "ai-code")]
#[command(about = "An AI coding agent in your terminal", long_about = None)]
struct Cli {
    #[arg(short, long, env = "AI_MODEL", default_value = "gpt-4o")]
    model: String,

    #[arg(short, long, env = "AI_API_URL", default_value = "https://api.openai.com/v1")]
    api_url: String,

    #[arg(short = 'k', long = "api-key", env = "AI_API_KEY")]
    api_key: Option<String>,

    #[arg(short, long)]
    prompt: Option<String>,

    #[arg(default_value = ".")]
    directory: PathBuf,
}

const SYSTEM_PROMPT: &str = r#"You are an AI coding agent running in the terminal. You help users with software engineering tasks.

You have access to tools for reading/writing files, running shell commands, and searching code.

Guidelines:
- Be concise and direct. Keep responses short.
- When reading files, use absolute paths.
- When running bash commands, explain what they do.
- Use the glob tool for finding files by pattern.
- Use the grep tool to search file contents.
- Never guess URLs unless confident they help with programming.
- Always follow the user's code conventions when making changes.

When the user asks you to make code changes:
1. First read the relevant files
2. Then use the write tool to make changes
3. Verify with bash if needed
"#;

fn create_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry
        .register(ReadTool::new())
        .register(WriteTool::new())
        .register(BashTool::new())
        .register(GlobTool::new())
        .register(GrepTool::new());
    registry
}

async fn run_repl(config: AgentConfig) -> anyhow::Result<()> {
    let mut tui = cli::Tui::new()?;

    let (crossterm_tx, mut crossterm_rx) = mpsc::unbounded_channel::<crossterm::event::Event>();

    std::thread::spawn(move || loop {
        if let Ok(ev) = crossterm::event::read() {
            if crossterm_tx.send(ev).is_err() {
                break;
            }
        }
    });

    let mut agent_handle: Option<tokio::task::JoinHandle<()>> = None;
    let mut agent_rx: Option<mpsc::UnboundedReceiver<AgentEvent>> = None;
    let mut is_agent_running = false;

    loop {
        tui.draw()?;

        if !is_agent_running {
            tokio::select! {
                Some(event) = crossterm_rx.recv() => {
                    tui.handle_input(event);

                    if tui.quit() {
                        break;
                    }

                    if let Some(input) = tui.take_input() {
                        let input_lower = input.to_lowercase();
                        if input_lower == "/quit" || input_lower == "/q" || input_lower == "/exit" {
                            break;
                        }

                        let (tx, rx) = mpsc::unbounded_channel();
                        let mut agent_loop = AgentLoop::new(config.clone(), create_registry());
                        agent_loop.add_user_message(input);
                        is_agent_running = true;

                        let handle = tokio::spawn(async move {
                            if let Err(e) = agent_loop.run(tx).await {
                                eprintln!("Agent error: {}", e);
                            }
                        });

                        agent_handle = Some(handle);
                        agent_rx = Some(rx);
                    }
                }
            }
        } else {
            tokio::select! {
                Some(event) = crossterm_rx.recv() => {
                    tui.handle_input(event);
                    if tui.quit() {
                        break;
                    }
                }
                event = async {
                    agent_rx.as_mut().unwrap().recv().await
                } => {
                    if let Some(ev) = event {
                        let is_done = matches!(ev, AgentEvent::Done | AgentEvent::Error(_));
                        if is_done {
                            is_agent_running = false;
                            agent_rx = None;
                            if let Some(handle) = agent_handle.take() {
                                handle.abort();
                            }
                        }
                        tui.handle_agent_event(ev);
                    } else {
                        is_agent_running = false;
                        agent_rx = None;
                        if let Some(handle) = agent_handle.take() {
                            handle.abort();
                        }
                    }
                }
            }
        }
    }

    if let Some(handle) = agent_handle.take() {
        handle.abort();
    }

    tui.shutdown()?;
    Ok(())
}

async fn run_single_prompt(config: AgentConfig, prompt: String) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();

    let mut agent = AgentLoop::new(config, create_registry());
    agent.add_user_message(prompt);

    let handle = tokio::spawn(async move {
        if let Err(e) = agent.run(tx).await {
            eprintln!("Agent error: {}", e);
        }
    });

    while let Some(event) = rx.recv().await {
        match &event {
            AgentEvent::AssistantDelta(delta) => {
                print!("{}", delta);
            }
            AgentEvent::ToolCallStart { name, .. } => {
                println!("\n[Running: {}]", name);
            }
            AgentEvent::Error(msg) => {
                eprintln!("\nError: {}", msg);
            }
            AgentEvent::Done => {
                println!();
            }
            _ => {}
        }
    }

    handle.await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let home_config = dirs::home_dir().map(|h| h.join(".config").join("ai-code").join(".env"));
    if let Some(ref path) = home_config {
        dotenvy::from_path(path).ok();
    }
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    std::env::set_current_dir(&cli.directory)?;

    let api_key = cli.api_key.unwrap_or_else(|| {
        env::var("AI_API_KEY")
            .or_else(|_| env::var("OPENAI_API_KEY"))
            .or_else(|_| env::var("ANTHROPIC_API_KEY"))
            .or_else(|_| env::var("DEEPSEEK_API_KEY"))
            .unwrap_or_default()
    });

    if api_key.is_empty() {
        let cfg_path = dirs::home_dir()
            .map(|h| h.join(".config").join("ai-code").join(".env"))
            .unwrap_or_default();
        eprintln!(
            "Error: No API key found.\n\
             \n  Add your key to {}:\n\
             \n    AI_API_KEY=sk-your-key\n    AI_MODEL=deepseek-chat\n    AI_API_URL=https://api.deepseek.com/v1\n\
             \n  Or set it via environment: export AI_API_KEY=sk-...\n\
             ",
            cfg_path.display()
        );
        std::process::exit(1);
    }

    let config = AgentConfig {
        system_prompt: SYSTEM_PROMPT.to_string(),
        model: cli.model,
        api_key,
        base_url: cli.api_url,
    };

    if let Some(prompt) = cli.prompt {
        run_single_prompt(config, prompt).await?;
    } else {
        run_repl(config).await?;
    }

    Ok(())
}

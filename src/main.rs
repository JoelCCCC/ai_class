mod agent;
mod cli;
mod llm;
mod tools;

use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use tokio::sync::{mpsc, oneshot};

use agent::{AgentConfig, AgentEvent, AgentLoop};
use cli::Command;
use llm::Message;
use tools::{
    bash::BashTool, fetch::FetchTool, glob::GlobTool, grep::GrepTool, read::ReadTool,
    registry::ToolRegistry, subagent::SubAgentTool, write::WriteTool,
};

#[derive(Parser)]
#[command(name = "ai-code")]
#[command(about = "An AI coding agent in your terminal", long_about = None)]
struct Cli {
    #[arg(short, long, env = "AI_MODEL", default_value = "gpt-4o")]
    model: String,

    #[arg(
        short,
        long,
        env = "AI_API_URL",
        default_value = "https://api.openai.com/v1"
    )]
    api_url: String,

    #[arg(short = 'k', long = "api-key", env = "AI_API_KEY")]
    api_key: Option<String>,

    #[arg(long, env = "AI_MAX_TOKENS", default_value = "8192")]
    max_tokens: u32,

    #[arg(long, env = "AI_SESSION", default_value = "default")]
    session: String,

    #[arg(
        short = 'l',
        long = "local",
        help = "Use local Ollama model (http://localhost:11434/v1)"
    )]
    local: bool,

    #[arg(long, help = "Start in planning mode (read-only research, no writes)")]
    plan: bool,

    #[arg(short, long)]
    prompt: Option<String>,

    #[arg(default_value = ".")]
    directory: PathBuf,
}

const SYSTEM_PROMPT: &str = r#"You are an AI coding agent running in the terminal. You help users with software engineering tasks.

Available tools:

read - Read a file. Required: file_path (absolute path). Optional: offset (1-indexed line), limit (max lines).
write - Write content to a file. Required: file_path (absolute path), content (string to write). Creates parent dirs.
bash - Execute a shell command. Required: command (string). Optional: workdir (directory). Non-interactive, no stdin.
glob - Find files by glob pattern. Required: pattern (e.g. '**/*.rs'). Optional: path (directory). .gitignore-aware.
grep - Search file contents by regex. Required: pattern (regex string). Optional: path, include (file filter). .gitignore-aware.
fetch - Fetch a URL via HTTP GET. Required: url (absolute URL, e.g. https://docs.rs/reqwest). Returns status code, content type, and body.
subagent - Spawn a sub-agent for an independent task. Required: task (string describing what to investigate). The sub-agent has all tools except write. Use for scouting, research, or parallel tasks.

Memory:
There is a memory file in the working directory. Use `pwd` via bash to find the absolute path, then check for .ai-code-memory.md in that directory using the read tool. When the user shares noteworthy information, preferences, or project decisions, write to this file so you remember them for future conversations. If the file doesn't exist, skip it and continue.

Rules:
- Always use absolute paths for file operations.
- Always respond in English, regardless of the user's language.
- If a tool returns an error, fix the arguments and retry.
- Do not call a tool name that is not in the list above.
- Be concise and direct.
- Use subagent for multi-step research tasks that can be done independently.
- Use fetch to look up documentation, crates.io versions, API references, etc.
"#;

fn create_registry(config: &AgentConfig) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry
        .register(ReadTool::new())
        .register(WriteTool::new())
        .register(BashTool::new())
        .register(GlobTool::new())
        .register(GrepTool::new())
        .register(FetchTool::new());
    let mut registry = Arc::new(registry);
    let sub_agent = SubAgentTool::new(config.clone(), &registry);
    Arc::get_mut(&mut registry).unwrap().register(sub_agent);
    registry
}

fn session_path(cwd: &Path, name: &str) -> PathBuf {
    let cwd_slug = cwd.to_string_lossy().replace('/', "__");
    let base = dirs::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("ai-code")
        .join("sessions")
        .join(&cwd_slug);
    let _ = std::fs::create_dir_all(&base);
    base.join(format!("{}.json", name))
}

fn load_session_into_tui(tui: &mut cli::Tui, messages: &[Message]) {
    for msg in messages {
        match msg.role.as_str() {
            "user" => {
                if let Some(ref content) = msg.content {
                    tui.push_user_message(content.clone());
                }
            }
            "assistant" => {
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        let name = tc.name.as_deref().unwrap_or("unknown");
                        tui.push_tool_start(name.to_string());
                    }
                }
                if let Some(ref content) = msg.content {
                    if !content.is_empty() {
                        tui.push_assistant_message(content.clone());
                    }
                }
            }
            "tool" => {
                if let Some(ref content) = msg.content {
                    let is_error = content.starts_with("Error");
                    tui.push_tool_result(content.clone(), is_error);
                }
            }
            _ => {}
        }
    }
}

fn configure_api_key(key: &str) {
    let config_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("ai-code")
        .join(".env");
    let _ = std::fs::create_dir_all(config_path.parent().unwrap());
    let mut content = std::fs::read_to_string(&config_path).unwrap_or_default();

    let key_line = format!("AI_API_KEY={}", key);
    if content.contains("AI_API_KEY=") {
        let mut updated = Vec::new();
        for line in content.lines() {
            if line.starts_with("AI_API_KEY=") {
                updated.push(key_line.as_str());
            } else {
                updated.push(line);
            }
        }
        content = updated.join("\n") + "\n";
    } else {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&key_line);
        content.push('\n');
    }
    let _ = std::fs::write(&config_path, content);
}

fn parse_model_name(name: &str) -> (String, Option<String>) {
    if let Some(stripped) = name.strip_prefix("[local] ") {
        (
            stripped.to_string(),
            Some("http://localhost:11434/v1".to_string()),
        )
    } else {
        (name.to_string(), None)
    }
}

fn extract_host(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .to_string()
}

async fn run_repl(
    mut config: AgentConfig,
    session_name: String,
    cwd: &Path,
    is_planning: bool,
) -> anyhow::Result<()> {
    let mut tui = cli::Tui::new()?;

    let host = extract_host(&config.base_url);
    tui.set_model_info(&format!("{} @ {}", config.model, host), &config.base_url);
    tui.set_planning(is_planning);

    let planning_prompt = format!(
        "{}\n\nCurrent mode: PLANNING — Research and plan only. Do not write any files. Use read, glob, grep, fetch, and subagent to gather information and produce a plan for the user to review.",
        SYSTEM_PROMPT
    );
    let execution_prompt = format!(
        "{}\n\nCurrent mode: EXECUTION — Full access to all tools including write. Implement the plan.",
        SYSTEM_PROMPT
    );

    if is_planning {
        config.system_prompt = planning_prompt.clone();
    } else {
        config.system_prompt = execution_prompt.clone();
    }

    let mut session_file = session_path(cwd, &session_name);

    enum ConfirmState {
        None,
        Waiting { tx: oneshot::Sender<bool> },
    }
    let mut confirm_state = ConfirmState::None;

    let (crossterm_tx, mut crossterm_rx) = mpsc::unbounded_channel::<crossterm::event::Event>();

    std::thread::spawn(move || loop {
        if let Ok(ev) = crossterm::event::read() {
            if crossterm_tx.send(ev).is_err() {
                break;
            }
        }
    });

    let registry = create_registry(&config);
    let mut agent_session = Some(AgentLoop::new(config.clone(), Arc::clone(&registry)));

    if session_file.exists() {
        if let Some(ref mut agent) = agent_session {
            match agent.load_from_file(&session_file) {
                Ok(()) => {
                    let msgs = agent.messages().to_vec();
                    load_session_into_tui(&mut tui, &msgs);
                    tui.push_system_message(format!("Loaded session: {}", session_file.display()));
                }
                Err(e) => {
                    tui.push_system_message(format!("Failed to load session: {}", e));
                }
            }
        }
    }

    type AgentJoinHandle = tokio::task::JoinHandle<(AgentLoop, anyhow::Result<String>)>;
    let mut agent_handle: Option<AgentJoinHandle> = None;
    let mut agent_rx: Option<mpsc::UnboundedReceiver<AgentEvent>> = None;
    let mut is_agent_running = false;
    let mut pending_model: Option<String> = None;

    loop {
        tui.draw()?;

        if !is_agent_running {
            tokio::select! {
                Some(event) = crossterm_rx.recv() => {
                    tui.handle_input(event);

                    if let Some(cmd) = tui.take_command() {
                        match cmd {
                            Command::SwitchSession(name) => {
                                let new_path = session_path(cwd, &name);
                                if new_path.exists() {
                                    let mut new_agent = AgentLoop::new(config.clone(), Arc::clone(&registry));
                                    if let Err(e) = new_agent.load_from_file(&new_path) {
                                        tui.push_system_message(format!("Failed to load session: {}", e));
                                    } else {
                                        let msgs = new_agent.messages().to_vec();
                                        tui.clear_chat();
                                        load_session_into_tui(&mut tui, &msgs);
                                        tui.push_system_message(format!(
                                            "Switched to session: {}",
                                            new_path.display()
                                        ));
                                        agent_session = Some(new_agent);
                                        let _ = std::fs::remove_file(&session_file);
                                        session_file = new_path;
                                    }
                                } else {
                                    tui.push_system_message(format!("Session not found: {}", name));
                                }
                            }
                            Command::SwitchModel(name) => {
                                let (model, new_url) = parse_model_name(&name);
                                if let Some(ref mut agent) = agent_session {
                                    agent.update_model(model.clone());
                                    if let Some(ref url) = new_url {
                                        agent.update_url(url.clone());
                                    }
                                } else {
                                    pending_model = Some(name.clone());
                                }
                                let real_url = new_url.as_deref().unwrap_or(&config.base_url);
                                tui.set_model_info(&format!("{} @ {}", model, extract_host(real_url)), real_url);
                                tui.push_system_message(format!("Switched model to: {}", model));
                            }
                            Command::ConfigureApi { key } => {
                                configure_api_key(&key);
                                tui.push_system_message(
                                    "API key updated in ~/.config/ai-code/.env. Restart to use new key.".into(),
                                );
                            }
                            Command::TogglePlanning => {
                                let new_mode = !tui.is_planning();
                                config.system_prompt = if new_mode {
                                    planning_prompt.clone()
                                } else {
                                    execution_prompt.clone()
                                };
                                tui.set_planning(new_mode);
                                tui.push_system_message(if new_mode {
                                    "Switched to planning mode — research and plan only.".into()
                                } else {
                                    "Switched to execution mode — full tool access.".into()
                                });
                            }
                            Command::ClearChat => {
                                let new_agent = AgentLoop::new(config.clone(), Arc::clone(&registry));
                                agent_session = Some(new_agent);
                                tui.clear_chat();
                                let _ = std::fs::remove_file(&session_file);
                            }
                            Command::Quit => {
                                tui.set_quit();
                            }
                        }
                    }

                    if tui.quit() {
                        if let Some(ref agent) = agent_session {
                            let _ = agent.save_to_file(&session_file);
                        }
                        break;
                    }

                    if tui.menu_active() {
                        continue;
                    }

                    if let Some(input) = tui.take_input() {
                        let input_lower = input.to_lowercase();
                        if input_lower == "/quit" || input_lower == "/q" || input_lower == "/exit" {
                            if let Some(ref agent) = agent_session {
                                let _ = agent.save_to_file(&session_file);
                            }
                            break;
                        }
                        if input_lower == "/clear" {
                            let new_agent = AgentLoop::new(config.clone(), Arc::clone(&registry));
                            agent_session = Some(new_agent);
                            tui.clear_chat();
                            let _ = std::fs::remove_file(&session_file);
                            continue;
                        }
                        if input_lower == "/help" {
                            tui.push_system_message(
                            "Ctrl+X: Menu  |  Commands: /help, /plan, /execute, /clear, /quit, /q, /exit  |  Keys: Enter=send, Esc=quit, PgUp/PgDn/mouse=scroll"
                                .into(),
                        );
                            continue;
                        }
                        if input_lower == "/plan" {
                            config.system_prompt = planning_prompt.clone();
                            tui.set_planning(true);
                            tui.push_system_message("Switched to planning mode — research and plan only.".into());
                            continue;
                        }
                        if input_lower == "/execute" {
                            config.system_prompt = execution_prompt.clone();
                            tui.set_planning(false);
                            tui.push_system_message("Switched to execution mode — full tool access.".into());
                            continue;
                        }

                        let (tx, rx) = mpsc::unbounded_channel();
                        let mut agent_loop = agent_session.take()
                            .unwrap_or_else(|| AgentLoop::new(config.clone(), Arc::clone(&registry)));
                        agent_loop.add_user_message(input);
                        is_agent_running = true;

                        let handle = tokio::spawn(async move {
                            let result = agent_loop.run(tx).await;
                            (agent_loop, result)
                        });

                        agent_handle = Some(handle);
                        agent_rx = Some(rx);
                    }
                }
            }
        } else {
            tokio::select! {
                Some(event) = crossterm_rx.recv() => {
                    match &confirm_state {
                        ConfirmState::Waiting { .. } => {
                            if let crossterm::event::Event::Key(key) = &event {
                                if key.kind == crossterm::event::KeyEventKind::Press {
                                    let allow = match key.code {
                                        crossterm::event::KeyCode::Char('y') |
                                        crossterm::event::KeyCode::Char('Y') => Some(true),
                                        crossterm::event::KeyCode::Char('n') |
                                        crossterm::event::KeyCode::Char('N') => Some(false),
                                        _ => None,
                                    };
                                    if let Some(allow) = allow {
                                        if let ConfirmState::Waiting { tx } = std::mem::replace(&mut confirm_state, ConfirmState::None) {
                                            let _ = tx.send(allow);
                                            tui.clear_confirm();
                                        }
                                    }
                                }
                            }
                        }
                        ConfirmState::None => {
                            tui.handle_input(event);
                            if let Some(cmd) = tui.take_command() {
                                match cmd {
                                    Command::Quit => tui.set_quit(),
                                    Command::ClearChat => {
                                        is_agent_running = false;
                                        agent_rx = None;
                                        if let Some(handle) = agent_handle.take() {
                                            handle.abort();
                                        }
                                        let new_agent = AgentLoop::new(config.clone(), Arc::clone(&registry));
                                        agent_session = Some(new_agent);
                                        tui.clear_chat();
                                        let _ = std::fs::remove_file(&session_file);
                                    }
                                    Command::SwitchModel(name) => {
                                        let (model, new_url) = parse_model_name(&name);
                                        if let Some(ref mut agent) = agent_session {
                                            agent.update_model(model.clone());
                                            if let Some(ref url) = new_url {
                                                agent.update_url(url.clone());
                                            }
                                        } else {
                                            pending_model = Some(name.clone());
                                        }
                                        let real_url = new_url.as_deref().unwrap_or(&config.base_url);
                                        tui.set_model_info(&format!("{} @ {}", model, extract_host(real_url)), real_url);
                                        tui.push_system_message(format!("Switched model to: {}", model));
                                    }
                                    Command::TogglePlanning => {
                                        let new_mode = !tui.is_planning();
                                        config.system_prompt = if new_mode {
                                            planning_prompt.clone()
                                        } else {
                                            execution_prompt.clone()
                                        };
                                        tui.set_planning(new_mode);
                                        tui.push_system_message(if new_mode {
                                            "Will switch to planning mode on next message".into()
                                        } else {
                                            "Will switch to execution mode on next message".into()
                                        });
                                    }
                                    _ => {}
                                }
                            }
                            if tui.quit() {
                                break;
                            }
                        }
                    }
                }
                event = async {
                    match agent_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                } => {
                    if let Some(ev) = event {
                        let is_done = matches!(ev, AgentEvent::Done | AgentEvent::Error(_));
                        if is_done {
                            is_agent_running = false;
                            agent_rx = None;
                            if let Some(handle) = agent_handle.take() {
                                match handle.await {
                                    Ok((loop_back, result)) => {
                                        let mut agent = loop_back;
                                        if let Some(ref model) = pending_model.take() {
                                            let (m, url) = parse_model_name(model);
                                            agent.update_model(m);
                                            if let Some(u) = url {
                                                agent.update_url(u);
                                            }
                                        }
                                        if let Err(e) = &result {
                                            tui.push_system_message(format!("Agent error: {}", e));
                                        }
                                        let _ = agent.save_to_file(&session_file);
                                        agent_session = Some(agent);
                                    }
                                    Err(join_err) => {
                                        eprintln!("Agent task panicked: {}", join_err);
                                        agent_session = Some(AgentLoop::new(config.clone(), Arc::clone(&registry)));
                                    }
                                }
                            }
                        }
                        match ev {
                            AgentEvent::WriteRequest { path, resume } => {
                                confirm_state = ConfirmState::Waiting { tx: resume };
                                tui.show_confirm(&path);
                            }
                            other => tui.handle_agent_event(other),
                        }
                    } else {
                        is_agent_running = false;
                        agent_rx = None;
                        if let Some(handle) = agent_handle.take() {
                            match handle.await {
                                Ok((loop_back, result)) => {
                                    let mut agent = loop_back;
                                    if let Some(ref model) = pending_model.take() {
                                        let (m, url) = parse_model_name(model);
                                        agent.update_model(m);
                                        if let Some(u) = url {
                                            agent.update_url(u);
                                        }
                                    }
                                    if let Err(e) = &result {
                                        tui.push_system_message(format!("Agent error: {}", e));
                                    }
                                    agent_session = Some(agent);
                                }
                                Err(_) => {
                                    agent_session = Some(AgentLoop::new(config.clone(), Arc::clone(&registry)));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(handle) = agent_handle.take() {
        handle.abort();
        let _ = handle.await;
    }

    if let Some(ref agent) = agent_session {
        let _ = agent.save_to_file(&session_file);
    }
    tui.shutdown()?;
    Ok(())
}

async fn run_single_prompt(config: AgentConfig, prompt: String) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();

    let registry = create_registry(&config);
    let mut agent = AgentLoop::new(config, Arc::clone(&registry));
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
            AgentEvent::WriteRequest { path, .. } => {
                eprintln!("\nWrite denied: {} (no TUI to confirm)", path);
            }
            _ => {}
        }
    }

    handle.await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let home_config = dirs::home_dir().map(|h| h.join(".config").join("ai-code").join(".env"));
    if let Some(ref path) = home_config {
        dotenvy::from_path(path).ok();
    }

    let mut cli = Cli::parse();

    if cli.local {
        if cli.api_url == "https://api.openai.com/v1" {
            cli.api_url = "http://localhost:11434/v1".into();
        }
        if cli.model == "gpt-4o" {
            cli.model = "llama3.2".into();
        }
    }

    let is_local = cli.api_url.starts_with("http://localhost:")
        || cli.api_url.starts_with("http://127.0.0.1:");

    std::env::set_current_dir(&cli.directory)?;

    let api_key = cli.api_key.unwrap_or_else(|| {
        env::var("AI_API_KEY")
            .or_else(|_| env::var("OPENAI_API_KEY"))
            .or_else(|_| env::var("ANTHROPIC_API_KEY"))
            .or_else(|_| env::var("DEEPSEEK_API_KEY"))
            .unwrap_or_default()
    });

    if api_key.is_empty() && !is_local {
        let cfg_path = dirs::home_dir()
            .map(|h| h.join(".config").join("ai-code").join(".env"))
            .unwrap_or_default();
        eprintln!(
            "Error: No API key found.\n\
             \n  Add your key to {}:\n\
             \n    AI_API_KEY=sk-your-key\n    AI_MODEL=deepseek-chat\n    AI_API_URL=https://api.deepseek.com/v1\n\
             \n  Or set it via environment: export AI_API_KEY=sk-...\n\
             \n  Or use a local model: cargo run -- --local\n\
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
        max_tokens: cli.max_tokens,
    };

    if let Some(prompt) = cli.prompt {
        run_single_prompt(config, prompt).await?;
    } else {
        let cwd = std::env::current_dir()?;
        run_repl(config, cli.session, &cwd, cli.plan).await?;
    }

    Ok(())
}

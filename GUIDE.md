# Codebase & Rust guide

This file explains how `ai-code` is structured and the Rust patterns it uses. Useful for contributors or Rust learners.

## Project structure

```
Cargo.toml          # dependencies (clap, ratatui, tokio, reqwest, serde, ...)
src/
├── main.rs         # CLI parsing, config, REPL dispatch, session mgmt
├── cli/
│   ├── mod.rs      # re-exports tui.rs
│   └── tui.rs      # ratatui terminal UI + interactive menu + write confirm
├── agent/
│   ├── mod.rs      # re-exports loop.rs
│   └── loop.rs     # AgentLoop: tool-calling loop, session save/load
├── llm/
│   ├── mod.rs      # re-exports client.rs
│   └── client.rs   # LlmClient: OpenAI-compatible SSE (line-buffered)
├── experts/
│   └── mod.rs      # expert profiles: auto-detect, load, list, defaults
├── teams/
│   └── mod.rs      # team configs: roles, routing, list, load, defaults
└── tools/
    ├── mod.rs      # Tool trait, ToolSpec, ToolCall, ToolResult
    ├── registry.rs # ToolRegistry: HashMap<name, Arc<dyn Tool>>
    ├── glob_match.rs # shared glob matching logic (extracted)
    ├── bash.rs     # bash tool (120s timeout)
    ├── read.rs     # read tool
    ├── write.rs    # write tool
    ├── glob.rs     # glob search tool
    ├── grep.rs     # regex search tool
    ├── fetch.rs    # HTTP GET tool
    └── subagent.rs # sub-agent spawning tool
```

Each module in `src/` has a `mod.rs` that declares submodules and re-exports their public types with `pub use`.

## Key Rust patterns

### 1. The Tool trait (`tools/mod.rs`)

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String>;
}
```

- `Send + Sync` — trait object must be thread-safe (registry is behind `Arc`)
- `#[async_trait]` — required because `async fn` in traits isn't stable in Rust 2021
- `anyhow::Result` — flexible error type used across the project

Seven tools implement this trait: `BashTool`, `ReadTool`, `WriteTool`, `GlobTool`, `GrepTool`, `FetchTool`, `SubAgentTool`. The subagent tool also receives an `active_team` shared state to enable role-based routing.

### The registry is now created in one shot (`main.rs`)

The old code used `Arc::get_mut` to register SubAgentTool after creating the Arc. The new code registers everything in one chain:

```rust
fn create_registry(
    config: &AgentConfig,
    active_team: Arc<RwLock<Option<TeamConfig>>>,
) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry
        .register(ReadTool::new())
        .register(WriteTool::new())
        .register(BashTool::new())
        .register(GlobTool::new())
        .register(GrepTool::new())
        .register(FetchTool::new())
        .register(SubAgentTool::new(config.clone(), active_team));
    Arc::new(registry)
}
```

`SubAgentTool` now takes an `Arc<RwLock<Option<TeamConfig>>>` — shared state for the active team. The `RwLock` allows concurrent reads (the subagent tool reads it during execution) while `Arc` enables shared ownership between the tool and `main.rs`.

### 2. Trait objects and `Arc` sharing (`tools/registry.rs`)

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}
```

- `HashMap<String, ...>` — tools looked up by name string
- `Arc<dyn Tool>` — heap-allocated, reference-counted trait object. `Arc` enables shared ownership across `AgentLoop` and sub-agents
- `.register::<T>()` accepts any `T: Tool + 'static`

The registry is wrapped in `Arc` from creation:

```rust
fn create_registry(config: &AgentConfig) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register(ReadTool::new()).register(WriteTool::new())...;
    let mut registry = Arc::new(registry);
    // SubAgentTool needs the config, but the registry is already Arc'd
    // so we use Arc::get_mut for the mutable reference (safe here because
    // there's only one reference at this point)
    Arc::get_mut(&mut registry)
        .expect("registry must have exactly one strong reference")
        .register(SubAgentTool::new(config.clone()));
    registry
}
```

### 3. Builder-pattern chaining (`main.rs`)

```rust
registry
    .register(ReadTool::new())
    .register(WriteTool::new())
    .register(BashTool::new());
```

`register` returns `&mut Self` to enable chaining:

```rust
pub fn register<T: Tool + 'static>(&mut self, tool: T) -> &mut Self {
    self.tools.insert(spec.name, Arc::new(tool));
    self
}
```

### 4. Enums with data variants (`agent/loop.rs`)

```rust
pub enum AgentEvent {
    AssistantDelta(String),
    ToolCallStart { name: String, _id: String },
    ToolCallEnd(ToolResult),
    Thinking,
    Done,
    Error(String),
    WriteRequest { path: String, resume: oneshot::Sender<bool> },
}
```

The `WriteRequest` variant uses a `oneshot::Sender` — a one-shot channel that sends exactly one value. The agent loop sends the event to the TUI, waits for the user to press y/n, then sends the boolean back through the channel. See pattern 8 below.

### 5. `tokio::select!` for concurrent input (`main.rs`)

```rust
tokio::select! {
    Some(event) = crossterm_rx.recv() => { tui.handle_input(event); }
    event = async { agent_rx.as_mut().unwrap().recv().await } => {
        tui.handle_agent_event(event);
    }
}
```

Waits for either user input or agent output, whichever arrives first — no polling.

### 6. Line-buffered SSE parsing (`llm/client.rs`)

The SSE stream parser uses a persistent buffer string with line-by-line extraction:

```rust
let mut buffer = String::new();
while let Some(chunk_result) = byte_stream.next().await {
    buffer.push_str(&String::from_utf8_lossy(&bytes));
    while let Some(newline_pos) = buffer.find('\n') {
        let line = buffer[..newline_pos].to_string();
        buffer = buffer[newline_pos + 1..].to_string();
        // parse "data: {json}" or "[DONE]"
    }
}
```

This handles SSE events split across TCP chunks — a persistent buffer accumulates bytes and extracts complete lines. The old code parsed each TCP chunk independently and failed when a `data:` line was split across two chunks.

The stream is sent through an `mpsc::unbounded_channel` inside the function:

```rust
let (event_tx, event_rx) = mpsc::unbounded_channel();
tokio::spawn(async move { /* parse loop sends to event_tx */ });
Ok(Box::pin(futures::stream::unfold(event_rx, |mut rx| async move {
    rx.recv().await.map(|e| (e, rx))
})))
```

`futures::stream::unfold` converts the `mpsc::Receiver` into a `Stream` by polling `recv()` repeatedly.

### 7. Tool call accumulation by index (`agent/loop.rs`, `llm/client.rs`)

The API sends tool call fields across multiple SSE chunks. The first chunk has `id` and `function.name`; later chunks only have `function.arguments`. Both `client.rs` and `loop.rs` accumulate by `index` (position in the array), not by `id`:

```rust
let index = tc["index"].as_u64().unwrap_or(0) as usize;
while tc_list.len() <= index {
    tc_list.push(ToolCallDelta { ... });
}
tc_list[index].arguments.push_str(args);
```

This avoids phantom duplicate entries that would occur if matching by `id` (since `id` is `None` in follow-up chunks).

### 8. `oneshot` channel for write confirmation (`agent/loop.rs`)

When the agent wants to write, it sends a `WriteRequest` event with a `oneshot::Sender<bool>` and then awaits the receiver:

```rust
let (resume_tx, resume_rx) = oneshot::channel();
let _ = tx.send(AgentEvent::WriteRequest {
    path: file_path.to_string(),
    resume: resume_tx,
});
let approved = resume_rx.await.unwrap_or(false);
```

Meanwhile in `main.rs`, the TUI shows "Allow write to /path? (y/n)" and listens for y/n keypresses. When the user presses y or n, the sender is invoked:

```rust
let allow = match key.code {
    KeyCode::Char('y' | 'Y') => Some(true),
    KeyCode::Char('n' | 'N') => Some(false),
    _ => None,
};
if let Some(allow) = allow {
    let _ = tx.send(allow);
}
```

This pattern suspends the agent's tool loop without blocking the TUI, because `oneshot::Receiver::await` yields control to the tokio runtime.

### 9. Session persistence with serde (`agent/loop.rs`)

Conversations are serialized as JSON arrays of `Message` and saved to `~/.config/ai-code/sessions/`:

```rust
pub fn save_to_file(&self, path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(&self.messages)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn load_from_file(&mut self, path: &Path) -> anyhow::Result<()> {
    self.messages = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    Ok(())
}
```

Since `Message` derives `Serialize` and `Deserialize`, the full conversation history (including tool calls, tool results, and user/assistant messages) round-trips cleanly through JSON.

### 10. Shared mutable state with `Arc<RwLock<>>` (`main.rs`, `tools/subagent.rs`)

The active team configuration is shared between `main.rs` (which can update it via menu/commands) and the `SubAgentTool` (which reads it during execution):

```rust
// main.rs — ownership
let active_team: Arc<RwLock<Option<TeamConfig>>> = Arc::new(RwLock::new(None));

// Switching teams (write)
*active_team.write().unwrap() = Some(team);

// Passing to subagent tool
SubAgentTool::new(config.clone(), active_team)

// In subagent execute (read)
if let Ok(guard) = self.active_team.read() {
    if let Some(ref team) = *guard {
        for role in &team.roles {
            // check if task starts with "role_name:"
        }
    }
}
```

`RwLock` allows multiple concurrent reads (multiple sub-agents can check the team simultaneously) while ensuring exclusive access for writes (when the user switches teams via the menu). The `Arc` enables shared ownership — the tool and main.rs each hold a reference.

### 11. SubAgent: AgentLoop as a tool with team routing (`tools/subagent.rs`)

The `SubAgentTool` creates a child `AgentLoop` with its own restricted registry and dedicated system prompt:

```rust
const SUBAGENT_BASE_PROMPT: &str = r#"You are an AI coding agent working on a specific task.
You have access to: read, bash, glob, grep, fetch.
Rules: ... You CANNOT write files."#;
```

When a team is active, the tool checks if the task string starts with `role_name:` and routes accordingly:

```rust
if let Some(stripped) = sub_task.strip_prefix(&format!("{}:", role.name)) {
    // Inject role instructions + optional expert profile into system prompt
    sub_config.system_prompt = format!(
        "{}\n\n--- Team role: {} ---\n{}",
        base, role.name, role.instructions
    );
    sub_task = stripped.trim().to_string();
    break;
}
```

This enables the workflow: `subagent(task: "plan: design the architecture")` → sub-agent gets the plan role's instructions and runs independently.

### 12. Expert auto-detection by file presence (`experts/mod.rs`)

```rust
pub fn detect_project(cwd: &Path) -> Option<(String, ExpertProfile)> {
    for (slug, profile) in list_profiles() {
        for file_pattern in &profile.match_files {
            if cwd.join(file_pattern).exists() {
                return Some((slug, profile));
            }
        }
    }
    None
}
```

Scans the current working directory for known files. Matching enables automatic expert activation on startup. Expert profiles are stored as JSON files in `~/.config/ai-code/experts/` and are user-editable.

### 13. Theme persistence via file I/O (`tui.rs`)

The app supports dark/light themes persisted to `~/.config/ai-code/theme`:

```rust
#[derive(Clone, Copy, PartialEq)]
pub enum Theme { Dark, Light }

struct ThemeColors { user: Color, ai: Color, tool: Color, ... }

impl ThemeColors {
    fn new(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self { user: Color::Cyan, ai: Color::Green, ... },
            Theme::Light => Self { user: Color::Blue, ai: Color::Green, ... },
        }
    }
}

fn load_theme() -> Theme { /* read from file */ }
fn save_theme(theme: Theme) { /* write to file */ }
```

The theme is loaded on startup and toggled via menu. All chat rendering methods reference `app.colors` instead of hardcoded colors. The status bar shows ☾ (dark) or ☀ (light).

### 14. Atomic spinner for smooth animation (`tui.rs`)

The spinner uses an `AtomicUsize` counter instead of timestamp-based calculation:

```rust
fn spinner_char() -> char {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static FRAME: AtomicUsize = AtomicUsize::new(0);
    let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    frames[FRAME.fetch_add(1, Ordering::Relaxed) % frames.len()]
}
```

`AtomicUsize` provides lock-free increment across threads. `Ordering::Relaxed` means no memory ordering guarantees — just atomic increment, which is sufficient for a visual spinner.

### 15. `#[serde(default)]` and `skip_serializing_if` for optional fields

```rust
pub struct ExpertProfile {
    pub name: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_files: Vec<String>,
}
```

- `#[serde(default)]` — if the field is missing in JSON, use `Vec::new()`
- `skip_serializing_if = "Vec::is_empty"` — don't write the field to JSON if it's empty

### 16. Generic dynamic dispatch pattern: `register<T: Tool + 'static>`

```rust
pub fn register<T: Tool + 'static>(&mut self, tool: T) -> &mut Self {
    let spec = tool.spec();
    self.tools.insert(spec.name, Arc::new(tool));
    self
}
```

The `<T: Tool + 'static>` bound means:
- `T` must implement `Tool`
- `T` must not borrow anything shorter than `'static` (it owns all its data)

The concrete type `T` is erased when wrapped in `Arc<dyn Tool>` — dynamic dispatch through a vtable.

### 17. Config via clap derive with env fallback (`main.rs`)

```rust
#[derive(Parser)]
struct Cli {
    #[arg(short, long, env = "AI_MODEL", default_value = "gpt-4o")]
    model: String,

    #[arg(long, env = "AI_MAX_TOKENS", default_value = "8192")]
    max_tokens: u32,

    #[arg(short = 'l', long = "local",
          help = "Use local Ollama model (http://localhost:11434/v1)")]
    local: bool,
}
```

Each field can be set via CLI flag, environment variable, or falls back to a default. The `--local` flag overrides `api_url` and `model` at runtime after parsing.

### 18. `try_join!` alternative: `oneshot` + `tokio::spawn` for sub-agent

The sub-agent runs in a separate tokio task, and the main agent loop waits for the result:

```rust
let agent_handle = tokio::spawn(async move { sub_agent.run(tx).await });
// drain channel events silently
tokio::spawn(async move { while rx.recv().await.is_some() {} });
// wait for completion
let result = agent_handle.await?;
```

The intermediate channel is drained by a second background task because the sub-agent emits `AgentEvent`s that nobody listens to in this context. Without draining, the channel would fill up and block.

## Tool reference

### How the LLM calls a tool

1. LLM responds with `tool_calls` in the SSE stream
2. Each tool call has: `id`, `name`, `arguments` (JSON string)
3. `loop.rs` accumulates tool call deltas across SSE chunks (by index)
4. If name is empty or unknown: produces an inline error tool result
5. If `write` and `deny_writes` is true (planning/subagent): rejects with error
6. If `write` and not denied: prompts user for y/n confirmation via oneshot channel
7. Otherwise: executes via `ToolRegistry`, appends result to history

### Tool validation (`loop.rs`)

Before execution, the loop validates:
- **Empty name** → inline error: "tool name is empty"
- **Unknown name** → inline error: "unknown tool 'foo'"
- **Invalid JSON arguments** → inline error with the parse error and raw string
- **Write in deny mode** → "Write denied: cannot write to ... (writes are disabled)"
- **Write not confirmed** → "Write denied by user: ..."

### read

**Schema:**
```json
{
  "name": "read",
  "parameters": {
    "type": "object",
    "properties": {
      "file_path": { "type": "string" },
      "offset": { "type": "integer" },
      "limit": { "type": "integer" }
    },
    "required": ["file_path"]
  }
}
```

**Execution:** reads file with `tokio::fs::read_to_string`, slices from `offset-1` for `limit` lines, returns with line numbers. Empty file → `"File is empty."`.

### write

**Schema:**
```json
{
  "name": "write",
  "parameters": {
    "type": "object",
    "properties": {
      "file_path": { "type": "string" },
      "content": { "type": "string" }
    },
    "required": ["file_path", "content"]
  }
}
```

**Execution:** creates parent dirs with `create_dir_all`, writes with `tokio::fs::write`. Before execution, the loop sends a `WriteRequest` event to the TUI and waits for user confirmation via `oneshot` channel. In planning mode or sub-agent mode, writes are denied without prompting.

### bash

**Schema:**
```json
{
  "name": "bash",
  "parameters": {
    "type": "object",
    "properties": {
      "command": { "type": "string" },
      "workdir": { "type": "string" }
    },
    "required": ["command"]
  }
}
```

**Execution:** runs `bash -c <command>` with a 120-second timeout:

```rust
let output = tokio::time::timeout(
    Duration::from_secs(BASH_TIMEOUT_SECS),
    child.output()
).await.map_err(|_| anyhow::anyhow!("Command timed out after 120s"))??;
```

### glob

**Schema:** pattern (glob string), optional path (directory). Uses `ignore::WalkBuilder` with `.gitignore` filters. Glob matching via shared `glob_match` function in `glob_match.rs`.

### grep

**Schema:** pattern (regex string), optional path (directory), include (file filter). Uses `regex::Regex` for matching, `ignore::WalkBuilder` for file walking, and the shared `glob_match` for include filtering.

### fetch

**Schema:**
```json
{
  "name": "fetch",
  "parameters": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "Absolute URL to fetch" }
    },
    "required": ["url"]
  }
}
```

**Execution** (`fetch.rs`):
- HTTP GET via `reqwest::get(url).await`
- Returns `Status: 200\nContent-Type: text/html\n\n<body...>`
- Body truncated to 8000 bytes with a `[truncated N total bytes]` note

### subagent

**Schema:**
```json
{
  "name": "subagent",
  "parameters": {
    "type": "object",
    "properties": {
      "task": {
        "type": "string",
        "description": "The task for the sub-agent. When a team is active, prefix with 'role_name:' to route to a specific team member."
      }
    },
    "required": ["task"]
  }
}
```

**Execution** (`subagent.rs`):

1. Reads the active team from `Arc<RwLock<Option<TeamConfig>>>`
2. If a team is active and task starts with `role_name:`:
   - Strips the prefix for the actual sub-task
   - Starts with `SUBAGENT_BASE_PROMPT` (dedicated prompt listing only read/bash/glob/grep/fetch)
   - Loads the role's optional expert profile and injects its prompt
   - Appends the role's `instructions` to the system prompt
3. If no team or no role prefix: uses plain SUBAGENT_BASE_PROMPT
4. Creates a child `ToolRegistry` (no write tool)
5. Creates a child `AgentLoop` with `deny_writes: true`
6. Runs the sub-agent in a spawned tokio task
7. Drains the sub-agent's event channel in a background task (prevents backpressure)
8. Returns the sub-agent's final text response

## Data flow

```
User input (TUI or -p flag)
    │
    ▼
┌──────────────────┐   HTTP SSE stream   ┌───────────┐
│  LlmClient       │◄───────────────────►│  LLM API  │
│  (line-buf)      │                     └───────────┘
└──────┬───────────┘
       │ text / tool calls
       ▼
┌──────────────────────┐
│  AgentLoop           │── validate name ──► inline error
│  (message hist)      │── parse JSON ────► inline error
│  + deny_writes       │── write request ─► oneshot confirm (y/n)
│  + session           │── subagent ──────► check active_team ──► role routing
│  + active_team (Arc) │── execute ──────► ToolRegistry ──► tool impl
└──────┬───────────────┘                              ├── read/write/bash
       │ final response                                ├── glob/grep
       ▼                                               ├── fetch
┌────────────────┐                                     └── subagent ──┐
│    Tui         │                                                        │
│  (ratatui)     │                                   ┌──────────────────┐ │
│  + menu        │                                   │  SubAgentLoop    │◄┘
│  + confirm     │                                   │  (own prompt)    │
│  + themes      │                                   │  + role instruct │
│  + sessions    │                                   │  + expert inject │
│  + experts     │                                   └──────────────────┘
│  + teams       │
└────────────────┘
```

## Dependency summary

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing with env fallback |
| `tokio` | async runtime, tasks, channels, timeouts |
| `reqwest` | HTTP client with SSE streaming |
| `ratatui` + `crossterm` | terminal UI framework |
| `serde` + `serde_json` | JSON serialization (API, sessions, experts) |
| `async-trait` | async methods in traits |
| `ignore` | `.gitignore`-aware file walking |
| `anyhow` | flexible error handling with context |
| `uuid` | generate IDs for tool calls |
| `regex` | grep tool pattern matching |
| `tui-textarea` | multiline text input widget |
| `dotenvy` | load `.env` files |
| `textwrap` | word-wrapping chat text |
| `futures` | `StreamExt`, `stream::unfold` for SSE |

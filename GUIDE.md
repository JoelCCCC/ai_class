# Codebase & Rust guide

This file explains how `ai-code` is structured and the Rust patterns it uses. Useful for contributors or Rust learners.

## Project structure

```
Cargo.toml          # dependencies (clap, ratatui, tokio, reqwest, serde, ...)
src/
├── main.rs         # entrypoint: CLI parsing, config, REPL dispatch
├── cli/
│   ├── mod.rs      # re-exports tui.rs
│   └── tui.rs      # ratatui terminal UI
├── agent/
│   ├── mod.rs      # re-exports loop.rs
│   └── loop.rs     # AgentLoop: tool-calling orchestration loop
├── llm/
│   ├── mod.rs      # re-exports client.rs
│   └── client.rs   # LlmClient: OpenAI-compatible SSE streaming
└── tools/
    ├── mod.rs      # Tool trait, ToolSpec, ToolCall, ToolResult
    ├── registry.rs # ToolRegistry: HashMap<name, Arc<dyn Tool>>
    ├── bash.rs     # bash tool
    ├── read.rs     # read tool
    ├── write.rs    # write tool
    ├── glob.rs     # glob tool
    └── grep.rs     # grep tool
```

Each module in `src/` has a `mod.rs` that declares submodules and re-exports their public types with `pub use`.

## Key Rust patterns

### 1. The Tool trait (`tools/mod.rs`)

Traits with `async_trait` — Rust's way of defining async interface methods:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String>;
}
```

- `Send + Sync` — the trait object must be safe to share across threads (the registry is behind `Arc`)
- `#[async_trait]` — required because `async fn` in traits isn't stable in Rust 2021
- `anyhow::Result` — flexible error type used across the project

Each tool (bash, read, write, glob, grep) implements this trait via `impl Tool for BashTool`.

### 2. Trait objects and dynamic dispatch (`tools/registry.rs`)

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}
```

- `HashMap<String, ...>` — tools looked up by name string
- `Arc<dyn Tool>` — heap-allocated, reference-counted trait object. `Arc` enables shared ownership across the `AgentLoop`
- `.register::<T>()` uses generics to accept any `T: Tool + 'static`

### 3. Builder-pattern chaining (`main.rs`)

```rust
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
```

`register` returns `&mut Self` to enable chaining.

### 4. Enums as state (`agent/loop.rs`)

```rust
pub enum AgentEvent {
    AssistantDelta(String),
    ToolCallStart { name: String, _id: String },
    ToolCallEnd(ToolResult),
    Thinking,
    Done,
    Error(String),
}
```

Rust enums can carry data — each variant has different fields (tuple, struct, or none). The TUI matches on these to update the display:

```rust
match event {
    AgentEvent::Thinking => { status = "Thinking..."; }
    AgentEvent::AssistantDelta(delta) => { chat.push(delta); }
    AgentEvent::ToolCallStart { name, .. } => { ... }
    ...
}
```

### 5. `tokio::select!` for concurrent input handling (`main.rs`)

```rust
tokio::select! {
    Some(event) = crossterm_rx.recv() => { tui.handle_input(event); }
    event = async { agent_rx.as_mut().unwrap().recv().await } => {
        tui.handle_agent_event(event);
    }
}
```

Waits for either user input or agent output, whichever arrives first — no polling or busy-waiting.

### 6. Streaming with `futures::StreamExt` (`llm/client.rs`)

```rust
let stream = response.bytes_stream()
    .map(|chunk| -> anyhow::Result<StreamEvent> {
        // parse SSE data lines from chunk bytes
    });
// consumed upstream as:
while let Some(chunk) = stream.next().await { ... }
```

Chains a transformation on a byte stream without buffering the entire response. Each chunk is parsed independently.

### 7. SSE parsing — per-chunk tool call indexing

The API sends tool calls across multiple SSE chunks. Each chunk's tool calls are indexed by position:

```rust
let index = tc["index"].as_u64().unwrap_or(0) as usize;
while tc_list.len() <= index {
    tc_list.push(ToolCallDelta { ... });
}
tc_list[index].arguments.push_str(args);
```

The accumulator in `loop.rs` mirrors this by merging across chunks using the same index, not by `id` (which is absent in follow-up chunks).

### 8. `#[serde(skip)]` for internal-only fields

```rust
pub struct ToolCallDelta {
    #[serde(skip)]
    pub index: Option<usize>,  // only used during accumulation, never serialized
}
```

Fields marked `#[serde(skip)]` are excluded from serde serialization/deserialization.

### 9. Config via environment and clap derive (`main.rs`)

```rust
#[derive(Parser)]
struct Cli {
    #[arg(short, long, env = "AI_MODEL", default_value = "gpt-4o")]
    model: String,
}
```

`clap`'s derive macro reads from CLI flags, environment variables, or defaults — all at once.

### 10. Owned string `Arc` sharing (`agent/loop.rs`)

```rust
pub struct AgentLoop {
    registry: Arc<ToolRegistry>,
    ...
}
```

The registry is created once and shared across all loop iterations. `Arc` provides reference-counted shared ownership without cloning the entire registry.

## Tool reference

Each tool is defined by a `ToolSpec` that the LLM receives as part of the API's `tools` parameter. The spec includes the tool's JSON Schema for arguments, which the LLM uses to construct valid tool calls.

### How the LLM calls a tool

1. LLM responds with `tool_calls` in the SSE stream
2. Each tool call has: `id` (API-assigned), `name` (tool name), `arguments` (JSON string)
3. `loop.rs` accumulates tool call deltas across SSE chunks (by index)
4. If name is empty or unknown: produces an inline error tool result
5. If valid: looks up the tool in `ToolRegistry` by name, executes with parsed arguments

### 1. read

**JSON schema** (what the LLM sees in the API spec):
```json
{
  "name": "read",
  "description": "Read a file. Required: file_path (absolute path). Optional: offset (1-indexed line), limit (max lines). Returns content with line numbers.",
  "parameters": {
    "type": "object",
    "properties": {
      "file_path": { "type": "string", "description": "Absolute path to the file to read" },
      "offset": { "type": "integer", "description": "Line number to start reading from (1-indexed)" },
      "limit": { "type": "integer", "description": "Maximum number of lines to read" }
    },
    "required": ["file_path"]
  }
}
```

**Execution** (`read.rs`):
- Reads the entire file with `tokio::fs::read_to_string`
- Splits into lines, slices from `(offset - 1)` for `limit` lines (or all remaining)
- Returns lines prefixed with 6-character line numbers: `     1: use std::io;`
- Empty file returns an empty string (no error)
- Missing file returns `anyhow::Error` (propagated to tool result as `is_error: true`)

**LLM calling convention:**
```json
{ "file_path": "/home/user/project/src/main.rs", "offset": 1, "limit": 50 }
```

### 2. write

**JSON schema:**
```json
{
  "name": "write",
  "description": "Write content to a file. Required: file_path (absolute path), content (string to write). Creates parent dirs.",
  "parameters": {
    "type": "object",
    "properties": {
      "file_path": { "type": "string", "description": "Absolute path to the file to write" },
      "content": { "type": "string", "description": "Content to write to the file" }
    },
    "required": ["file_path", "content"]
  }
}
```

**Execution** (`write.rs`):
- Extracts parent directory from `file_path`, creates it with `create_dir_all` (no error if exists)
- Writes content with `tokio::fs::write` (overwrites if exists)
- Returns `Successfully wrote file: /path/to/file`
- Empty `file_path` returns error: `file_path is required`
- `content` is optional in practice (defaults to `""`) — writing an empty string is valid

**Edge cases:**
- File already exists: silently overwritten
- Parent directory doesn't exist: created automatically
- Write to read-only path: returns permission error

### 3. bash

**JSON schema:**
```json
{
  "name": "bash",
  "description": "Execute a shell command. Required: command (string). Optional: workdir (directory). Non-interactive, no stdin.",
  "parameters": {
    "type": "object",
    "properties": {
      "command": { "type": "string", "description": "The bash command to execute" },
      "workdir": { "type": "string", "description": "Optional working directory for the command" }
    },
    "required": ["command"]
  }
}
```

**Execution** (`bash.rs`):
- Runs `bash -c <command>` on Linux/macOS, `cmd /C <command>` on Windows
- Sets working directory via `child.current_dir(workdir)` (defaults to `"."`)
- Captures stdout and stderr separately, concatenates both into the result
- If both stdout and stderr are empty, returns the exit code
- No stdin pipe — command cannot read from stdin (fails silently or hangs)

**Edge cases:**
- Non-zero exit: stdout/stderr are still returned, no error — the tool never produces `is_error` from exit codes
- Missing `command`: returns `anyhow::Error` → `is_error: true`
- Background processes (`&`): the child is not waited on; output may be empty
- Interactive commands (like `vim`, `top`): hang indefinitely (no PTY, no stdin)
- Long-running commands: no timeout — tokio process runs until completion
- Working dir doesn't exist: returns `Command exited with code: 1` (bash stderr may say "no such file")

### 4. glob

**JSON schema:**
```json
{
  "name": "glob",
  "description": "Find files by glob pattern. Required: pattern (e.g. '**/*.rs'). Optional: path (directory). .gitignore-aware.",
  "parameters": {
    "type": "object",
    "properties": {
      "pattern": { "type": "string", "description": "Glob pattern to match (e.g. '**/*.rs')" },
      "path": { "type": "string", "description": "Directory to search in" }
    },
    "required": ["pattern"]
  }
}
```

**Execution** (`glob.rs`):
- Walks directory tree using `ignore::WalkBuilder` with `.gitignore` filters
- For each file, checks if the full path (relative to search root) matches the glob pattern
- Uses a custom `glob_match` function (not a library) supporting `*` (any chars) and `?` (single char)
- `**` is NOT special — treated as two literal `*` characters. So `**/*.rs` matches by the combination of `**/` matching double-dot and `*.rs`
- Returns newline-separated paths, or `No matching files found.`

**How `glob_match` works** (`glob.rs:71-104`):
- Two-pointer scan: `pi` on pattern, `si` on path string
- On `*`: mark backtrack position, advance pattern pointer
- On `?`: advance both pointers (match any single char)
- On exact char: advance both if equal
- On mismatch: backtrack to last `*` and try matching one more char
- At end: skip trailing `*` in pattern, return true if pattern is consumed

**Limitations:**
- No `**` for recursive matching (the conventional `**/*.rs` works accidentally because `**/` resolves as any chars including `/`)
- No character classes `[abc]`, no alternation `{a,b}`, no negation `!`
- No brace expansion

### 5. grep

**JSON schema:**
```json
{
  "name": "grep",
  "description": "Search file contents by regex. Required: pattern (regex string). Optional: path, include (file filter e.g. '*.rs'). .gitignore-aware.",
  "parameters": {
    "type": "object",
    "properties": {
      "pattern": { "type": "string", "description": "Regex pattern to search for" },
      "path": { "type": "string", "description": "Directory to search in" },
      "include": { "type": "string", "description": "File pattern to filter (e.g. '*.rs')" }
    },
    "required": ["pattern"]
  }
}
```

**Execution** (`grep.rs`):
- Builds a `regex::Regex` from the pattern (returns `anyhow::Error` on invalid regex)
- Walks directory tree with `ignore::WalkBuilder` (`.gitignore`-aware)
- If `include` is set, filters files by the same custom `glob_match` function (applied to filename only, not full path)
- Reads each matching file with `tokio::fs::read_to_string`, scans line by line with `regex.is_match(line)`
- Returns results as `path/to/file.rs:42: let x = 1;` — one per match, or `No matches found.`
- Binary/unreadable files: silently skipped (`Err(_) => continue`)

**Regex notes:**
- Uses the `regex` crate (not PCRE) — no lookahead/lookbehind, no backreferences
- Pattern is an arbitrary regex string, escaped by the LLM when searching for special chars

**`include` filtering:**
- Applied to the filename only (not the full path)
- Uses the same `glob_match` as the glob tool (duplicated code in `grep.rs:8-41`)
- Example: `include: "*.rs"` matches `main.rs` but not `Cargo.toml`

### Tool call validation (`loop.rs:155-177`)

Before executing, the loop validates every tool call:

```rust
if name.is_empty() || !tool_names.contains(&name) {
    // produces inline error: "Error: tool name is empty" or "Error: unknown tool 'foo'"
    // includes "Available tools: read, write, bash, glob, grep."
    // tool result has is_error: true
    continue;
}
```

This catches two failure modes:
- **Empty name** — the LLM returns a tool call delta without `function.name` (malformed)
- **Unknown name** — the LLM hallucinates a tool that doesn't exist

Both produce a tool result with `is_error: true` and a clear message listing the available tools. The tool result is appended to the conversation, so the LLM sees the error and can retry.

### ToolSpec → API serialization (`client.rs:99-114`)

When sending tools to the API, each spec is wrapped in OpenAI's `function` format:

```rust
{
    "type": "function",
    "function": {
        "name": t.name,
        "description": t.description,
        "parameters": t.parameters
    }
}
```

The `parameters` JSON object is the JSON Schema from each tool's `spec()`. The API uses this schema to validate the LLM's generated arguments before returning them.

## Data flow

```
main.rs                    cli/tui.rs
   │                           ▲
   ▼ user message              │ final text
agent/loop.rs ── HTTP ──► llm/client.rs ── SSE stream ──► LLM API
   │                           ▲
   │ tool call                 │ tool result
   ▼                           │
tools/registry.rs ──────► tool impl (bash, read, ...)
```

1. `main.rs` parses CLI, creates `ToolRegistry` and `AgentConfig`
2. User types a message in the TUI or passes `-p` flag
3. `AgentLoop::run` enters the tool loop (max 25 iterations):
   - Calls `LlmClient::stream_chat` with message history + tool definitions
   - Streams SSE response, accumulating text and tool call deltas by index
   - If tool calls arrive: validates names, executes via `ToolRegistry`, appends results to history, loops
   - If text only: exits loop, sends response to TUI
4. TUI renders the conversation

## Dependency summary

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing |
| `tokio` | async runtime, tasks, channels |
| `reqwest` | HTTP client with SSE streaming |
| `ratatui` + `crossterm` | terminal UI framework |
| `serde` + `serde_json` | JSON serialization for API calls |
| `async-trait` | async methods in traits |
| `ignore` | `.gitignore`-aware file walking |
| `anyhow` | flexible error handling |
| `uuid` | generate IDs for tool calls |
| `regex` | grep tool pattern matching |
| `tui-textarea` | multiline text input widget |
| `dotenvy` | load `.env` files |
| `textwrap` | word-wrapping chat text |
| `futures` | `StreamExt` for SSE streaming |

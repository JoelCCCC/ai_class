# ai-code

A terminal-based AI coding agent. It wraps an LLM (OpenAI-compatible API) with tools to read, write, search, and run code — all from the command line.

![screenshot](https://img.shields.io/badge/terminal-TUI-blue)

## Quick start

```bash
cp .env.example .env
# edit .env and set AI_API_KEY
cargo run
```

## Setup

Copy `.env.example` to `.env` and set your API key. The app looks for the key in this order:

1. `AI_API_KEY` in `.env` or environment
2. `~/.config/ai-code/.env`
3. `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or `DEEPSEEK_API_KEY`

Configure the model and API URL in `.env`:

```
AI_MODEL=deepseek-chat
AI_API_URL=https://api.deepseek.com/v1
```

Default is `gpt-4o` via `https://api.openai.com/v1`.

## Usage

### REPL mode (default)

```bash
cargo run
```

- **Enter** — send message
- **Esc** — quit
- **PgUp** / **PgDn** — scroll chat
- **Mouse scroll** — scroll chat

### One-shot mode

```bash
cargo run -- -p "find all TODO comments in src/"
```

### Options

| Flag | Short | Env | Default |
|------|-------|-----|---------|
| `--model` | `-m` | `AI_MODEL` | `gpt-4o` |
| `--api-url` | `-u` | `AI_API_URL` | `https://api.openai.com/v1` |
| `--api-key` | `-k` | `AI_API_KEY` | — |
| `--prompt` | `-p` | — | — |
| `--directory` | — | — | `.` |

## What it can do

The agent has five tools:

| Tool | What it does |
|------|-------------|
| `read` | Read a file with optional offset/limit |
| `write` | Write content to a file (creates parent dirs) |
| `bash` | Run a shell command (no PTY, no stdin) |
| `glob` | Find files by glob pattern (`.gitignore`-aware) |
| `grep` | Search file contents by regex (`.gitignore`-aware) |

## How it works

```
User input
    │
    ▼
┌──────────────┐   HTTP SSE stream   ┌───────────┐
│  LlmClient   │◄───────────────────►│  LLM API  │
└──────┬───────┘                     └───────────┘
       │ text / tool calls
       ▼
┌──────────────┐   execute tool   ┌──────────────┐
│  AgentLoop   │─────────────────►│ ToolRegistry │
│  (history)   │◄──── result ────└──────────────┘
└──────┬───────┘
       │ final response
       ▼
┌──────────────┐
│    Tui       │
│  (ratatui)   │
└──────────────┘
```

When you send a message, `AgentLoop` enters a tool-calling loop (max 25 rounds):

1. Send conversation history + tool definitions to the LLM API as an SSE stream
2. Stream the response, accumulating text content and tool call deltas in parallel
3. If the response has tool calls:
   - Validate tool names (reject empty or unknown names with an error)
   - Execute each valid tool call via `ToolRegistry`
   - Append tool results to the message history and loop back to step 1
4. If the response has text only: display it as the final answer

## Architecture

```
src/
├── main.rs          # CLI, config, REPL/single-prompt dispatch
├── cli/tui.rs       # ratatui terminal UI (input, scroll, render)
├── agent/loop.rs    # tool-calling loop — core orchestration
├── llm/client.rs    # OpenAI-compatible SSE streaming client
└── tools/           # Tool trait + 5 implementations
    ├── registry.rs  # tool lookup and dispatch
    ├── bash.rs
    ├── read.rs
    ├── write.rs
    ├── glob.rs
    └── grep.rs
```

No external LLM SDK — the agent speaks SSE directly to any OpenAI-compatible API.

## Requirements

- Rust 2021 edition
- A terminal that supports raw mode (most do)

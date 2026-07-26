# ai-code

A terminal-based AI coding agent. It wraps an LLM (OpenAI-compatible API) with tools to read, write, search, fetch, and run code — all from the command line.

Supports local models (Ollama), session persistence, expert profiles, team collaboration, planning mode, theme switching, and a full interactive menu.

## Quick start

```bash
cp .env.example .env
# edit .env and set AI_API_KEY and AI_MODEL
cargo run
```

## Setup

Copy `.env.example` to `.env` and set your API key. The app looks for the key in this order:

1. `AI_API_KEY` in `.env` or environment
2. `~/.config/ai-code/.env`
3. `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or `DEEPSEEK_API_KEY`

Optionally configure the model and API URL:

```env
AI_MODEL=deepseek-v4-pro
AI_API_URL=https://api.deepseek.com/v1
AI_MAX_TOKENS=8192
```

Defaults are `gpt-4o` via `https://api.openai.com/v1` and `8192` max tokens.

### Local models (Ollama)

```bash
cargo run -- --local
# or specify a model:
cargo run -- --local --model codellama
```

No API key needed for local models. The app skips the key check when the URL points to `localhost`.

## Usage

### REPL mode (default)

```bash
cargo run
```

| Key | Action |
|-----|--------|
| **Enter** | Send message |
| **Esc** | Quit |
| **Ctrl+X** | Open interactive menu |
| **PgUp** / **PgDn** | Scroll chat |
| **Mouse scroll** | Scroll chat |

### Interactive menu (Ctrl+X)

The menu provides quick access without typing commands:

- **Switch Session** — load a previous conversation (sessions auto-save per directory)
- **Switch Model** — pick from local Ollama models or cloud models (auto-detected from API URL)
- **Switch Expert** — switch between expert profiles
- **Select Team** — activate a team configuration for role-based sub-agent delegation
- **Configure API** — enter a new API key (saved to `~/.config/ai-code/.env`)
- **Toggle Planning Mode** — switch between read-only research and full execution
- **Toggle Theme** — switch between dark and light color themes (persisted)
- **Clear Chat** — start a fresh conversation
- **Help** — keyboard shortcuts reference

### Chat commands

| Command | Action |
|---------|--------|
| `/help` | Show available commands |
| `/clear` | Clear current session |
| `/plan` | Switch to planning mode (read-only, no writes) |
| `/execute` | Switch to execution mode (full tool access) |
| `/expert list` | List available expert profiles |
| `/expert <name>` | Switch to a specific expert |
| `/expert general` | Switch to no specialization |
| `/quit` or `/q` or `/exit` | Exit |

### Planning mode

Start in planning mode: `cargo run -- --plan`

In planning mode, the agent can read, search, fetch, and use sub-agents but cannot write files. When the agent attempts a write, it sees:

```
Write denied: writes are disabled in planning mode.
```

Use `/execute` to switch to full access or toggle via the menu.

### Write confirmation

Every file write prompts for confirmation in the status bar:

```
Allow write to /path/to/file? (y/n)
```

Press **y** to allow, **n** to deny. In one-shot mode (`-p`), writes are automatically denied.

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
| `--max-tokens` | — | `AI_MAX_TOKENS` | `8192` |
| `--session` | — | `AI_SESSION` | `default` |
| `--local` | `-l` | — | — |
| `--plan` | — | — | — |
| `--prompt` | `-p` | — | — |
| `--directory` | — | — | `.` |

`--local` sets API URL to `http://localhost:11434/v1` and model to `llama3.2` unless overridden.
`--plan` starts in read-only planning mode.

## Tools

The agent has seven tools:

| Tool | What it does |
|------|-------------|
| `read` | Read a file with optional offset/limit |
| `write` | Write content to a file (prompts for confirmation) |
| `bash` | Run a shell command (120s timeout, no stdin) |
| `glob` | Find files by glob pattern (`.gitignore`-aware) |
| `grep` | Search file contents by regex (`.gitignore`-aware) |
| `fetch` | HTTP GET a URL (for docs, APIs, crates.io, etc.) |
| `subagent` | Spawn a sub-agent for independent research (no write access) |

### fetch

Fetches a URL via HTTP GET. Returns status code, content type, and body (truncated to 8000 bytes). Use for looking up documentation, checking API responses, or browsing crates.io.

### subagent

Spawns a child agent that runs independently with its own tool loop. The sub-agent has `read`, `bash`, `glob`, `grep`, and `fetch` (but NOT `write`). The sub-agent uses its own dedicated system prompt that lists only its available tools.

The sub-agent's final text response is returned as the tool result.

**Team mode:** When a team is active, prefix the task with `role_name:` to route the sub-agent to a specific team member. The sub-agent inherits that role's instructions and optional expert profile. See "Teams" below.

**Without a team:** Use subagent for independent research tasks that don't need write access:

```
subagent(task: "Read all files in src/tools/ and summarize each tool's purpose")
```

### bash

Runs `bash -c <command>`. Has a 120-second timeout — commands that exceed it return an error. No PTY, no stdin — interactive commands (vim, top) will hang until timeout.

## Teams

Teams allow role-based delegation of sub-agents. A team configuration defines named roles, each with optional expert profile and instructions. When a team is active, the `subagent` tool checks if the task string starts with `role_name:` and automatically assigns the task to that role.

### Default team: Software Engineering Team

When you activate the SE team (via menu or by placing a config file), the following roles become available:

| Role | Expert | Purpose |
|------|--------|---------|
| `plan` | — | Project planner — research, design, produce plans (no writes) |
| `read_code` | Rust | Codebase architect — read and understand existing code |
| `backend` | Rust | Backend developer — implement server-side changes |
| `frontend` | Next.js | Frontend developer — implement UI components |

**Usage with team active:**

```
subagent(task: "plan: design the database schema for a todo app")
subagent(task: "read_code: read src/agent/loop.rs and explain the tool loop")
subagent(task: "backend: implement the todo API endpoints")
subagent(task: "frontend: create the todo list React component")
```

The sub-agent automatically gets:
- The role's instructions injected into its system prompt
- The role's expert profile (if configured) loaded for domain-specific conventions
- Only read/bash/glob/grep/fetch tools (no write)

### Creating a custom team

Teams are JSON files in `~/.config/ai-code/teams/<slug>.json`:

```json
{
  "name": "My Team Name",
  "prompt": "You are leading a team...",
  "roles": [
    {
      "name": "role_name",
      "expert": "rust",
      "instructions": "You are the X. Do Y..."
    },
    {
      "name": "another_role",
      "expert": null,
      "instructions": "You are the Z..."
    }
  ]
}
```

- `name` — display name for the menu
- `prompt` — injected into the main agent's system prompt, describes the team workflow
- `roles` — array of role definitions
  - `name` — role identifier used as `role_name:` prefix in subagent tasks
  - `expert` — optional slug of an expert profile to load for this role
  - `instructions` — injected into the sub-agent's system prompt when this role is invoked

### Activating a team

- Via menu: Ctrl+X → Select Team → pick a team
- The status bar shows when a team is active
- Switch `_none` to deactivate team mode
- Switching teams clears the chat and starts a fresh session

## Sessions

Conversations are auto-saved to `~/.config/ai-code/sessions/<cwd_slug>/<session_name>.json` after each agent response. Sessions are specific to the current working directory.

- Default session name: `default`
- Customize with `--session my-session-name`
- Switch between sessions via the menu (Ctrl+X → Switch Session)
- `/clear` deletes the session file and starts fresh

Session files are plain JSON arrays of messages.

## Expert profiles

Expert profiles inject specialized system prompts to tailor the agent's behavior for specific frameworks or languages. They live in `~/.config/ai-code/experts/<slug>.json` as JSON files:

```json
{
  "name": "Rust Systems Expert",
  "prompt": "You are an expert Rust developer...",
  "match_files": ["Cargo.toml", "Cargo.lock"]
}
```

If a `match_files` file exists in the project directory, that expert is auto-activated on startup. Auto-detection supports:
- **Rust** (Cargo.toml) — Rust conventions, serde, tokio, clippy
- **Next.js** (next.config.\*) — App Router, RSC, Tailwind, TypeScript
- **Python** (requirements.txt, pyproject.toml, setup.py) — Pydantic, FastAPI, type hints

Experts can be overridden at any time with `/expert <slug>` or via the menu. User profiles in `~/.config/ai-code/experts/` take precedence over defaults.

### Creating a custom expert

```bash
mkdir -p ~/.config/ai-code/experts
cat > ~/.config/ai-code/experts/my-expert.json << 'EOF'
{
  "name": "My Framework Expert",
  "prompt": "You are an expert in my framework. Follow these conventions: ...",
  "match_files": ["my-framework.config.js"]
}
EOF
```

Then activate with `/expert my-expert` or via the menu.

### Expert + team interaction

When a team role has an `expert` field, activating that role in a sub-agent automatically loads the expert profile. This means `backend:` role with `expert: "rust"` spawns a sub-agent that is both a backend developer AND a Rust expert.

## Theme

The app supports dark and light color themes. The theme is persisted to `~/.config/ai-code/theme`.

- Toggle via menu (Ctrl+X → Toggle Theme)
- The status bar shows the current theme (☾ for dark, ☀ for light)
- Theme affects chat messages, labels, status bar, and menu colors
- The persisted file is a plain text file containing `dark` or `light`

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
┌───────────────────┐   execute   ┌──────────────┐
│  AgentLoop        │────────────►│ ToolRegistry │
│  (history)        │◄── result ──└──────┬───────┘
│  + sessions       │                    ├── read/write/bash
│  + experts        │                    ├── glob/grep
│  + planning       │                    ├── fetch
│  + teams (shared) │                    └── subagent → AgentLoop
└───────┬───────────┘                              │
        │ final response                            └─ role routing
        ▼                                             │
┌──────────────┐                                      ├─ expert load
│    Tui       │                                      └─ instructions
│  (ratatui)   │
│  + menu      │
│  + confirm   │
│  + themes    │
└──────────────┘
```

The `AgentLoop` enters a tool-calling loop (max 25 rounds):

1. Send conversation history + tool definitions to the LLM API as an SSE stream
2. Stream the response, accumulating text and tool call deltas by index across SSE chunks
3. If the response has tool calls:
   - Validate tool names (reject empty or unknown names)
   - For `write` calls: prompt the user for y/n confirmation (or deny in planning/subagent mode)
   - For `subagent` calls: check if a team is active and route to the appropriate role
   - Execute valid tool calls via `ToolRegistry`
   - Append tool results and loop back to step 1
4. If the response has text only: display it as the final answer, auto-save the session

## Architecture

```
src/
├── main.rs            # CLI, config, REPL dispatch, session mgmt
├── cli/tui.rs         # ratatui terminal UI + interactive menu + themes
├── agent/loop.rs      # tool-calling loop (planning, confirm, sessions)
├── experts/mod.rs     # expert profiles (auto-detect + load)
├── teams/mod.rs       # team configurations (roles, routing)
├── llm/client.rs      # OpenAI-compatible SSE streaming (line-buffered)
└── tools/
    ├── mod.rs         # Tool trait, ToolSpec, ToolCall, ToolResult
    ├── registry.rs    # tool lookup and dispatch
    ├── glob_match.rs  # shared glob matching logic
    ├── bash.rs        # shell command (120s timeout)
    ├── read.rs        # file reader
    ├── write.rs       # file writer
    ├── glob.rs        # glob search
    ├── grep.rs        # regex search
    ├── fetch.rs       # HTTP GET
    └── subagent.rs    # sub-agent launcher with team routing
```

No external LLM SDK — the agent speaks SSE directly to any OpenAI-compatible API.

## Requirements

- Rust 2021 edition
- A terminal that supports raw mode (most do)
- (Optional) [Ollama](https://ollama.ai) for local models

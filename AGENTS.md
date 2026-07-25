# ai-code

Single-crate Rust TUI agent that wraps an LLM with file/command tools.

## Build & run

```bash
cargo build
cargo run -- --help          # see all CLI flags
cargo run -- -p "your prompt"  # one-shot mode (no TUI)
cargo run                     # TUI REPL mode
cargo test                    # one test in tools::glob
```

## Setup

Copy `.env.example` → `.env` and set `AI_API_KEY`. Falls back to `~/.config/ai-code/.env`, then `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `DEEPSEEK_API_KEY`.

```bash
cp .env.example .env
```

## Architecture

- **`src/main.rs`** — entrypoint, `Cli` parser (clap), `SYSTEM_PROMPT`, tool registry wiring, REPL vs single-prompt dispatch
- **`src/cli/`** — ratatui + crossterm TUI (`tui.rs`) or `-p` mode
- **`src/agent/loop.rs`** — `AgentLoop`: streaming tool-calling loop (max 25 iterations), manages message history
- **`src/llm/client.rs`** — OpenAI-compatible streaming client over SSE, no SDK dependency
- **`src/tools/`** — `Tool` trait, 5 tools (`bash`, `read`, `write`, `glob`, `grep`); all use `.gitignore`-aware walking (`ignore` crate)

## Key details

- `.env` is gitignored; keys never committed
- TUI: `Enter` to send, `Esc` to quit, `PgUp`/`PgDn`/mouse scroll
- `bash` tool runs non-interactive `bash -c` — no stdin, no PTY
- `grep` uses custom glob-matching (duplicated code in `glob.rs` and `grep.rs`)
- No lint/formatter config; `cargo fmt` / `cargo clippy` use defaults
- `dotenvy` loads config from project `.env` then `~/.config/ai-code/.env`

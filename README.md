# shrimp

Coding agent for local LLMs. Ollama & LM Studio. Tree-sitter indexing, streaming TUI

<img width="800" height="536" alt="ScreenRecording2026-06-06at16 57 20-ezgif com-video-to-gif-converter" src="https://github.com/user-attachments/assets/66a964b0-cbf3-4235-b5c6-711191522135" />

## What it does

- Runs on **Ollama** or **LM Studio** on your machine
- Indexes your repo with **tree-sitter** (symbols, fast lookup)
- **Streaming TUI** with syntax-colored code and markdown replies
- **Headless mode** for scripts and CI (`shrimp run`)

## Install

```bash
git clone https://github.com/ddoemonn/shrimp.git
cd shrimp
cargo install --path crates/shrimp-cli
```

## Usage

```bash
shrimp                          # open TUI: pick provider, model, chat
shrimp run -P "create hello.py" # one-shot prompt, no TUI
shrimp index                    # index the current repo
```

Providers: **Ollama** at `localhost:11434` (configurable via `OLLAMA_HOST` environment variable), **LM Studio** at `localhost:1234` (configurable via `LM_STUDIO_HOST` environment variable).

| key               | action                          |
| ----------------- | ------------------------------- |
| Enter             | send message                    |
| ↑↓ / PgUp/PgDn    | scroll transcript               |
| ⌘C / Ctrl+Shift+C | copy transcript                 |
| End               | jump to live stream             |
| /model /provider  | switch model or provider        |
| /undo /reindex    | undo last edit or rebuild index |
| Esc               | quit                            |

## Config

Settings live in `.shrimp/config.toml`. Every field is optional.

```toml
provider = "ollama"
base_url = "http://hub:11434" # custom base URL (optional)
model = "gemma4:12b"
auto_approve = true
max_context_tokens = 8192
```

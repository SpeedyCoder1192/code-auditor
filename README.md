# code-auditor

Finds bugs and security holes in your code with a language model and suggests
patches you can apply with one key. By default it runs on your own machine
through [Ollama](https://ollama.com), so it costs nothing, works offline and
your code stays local. If you have an OpenAI-compatible or Gemini key, it can
use that instead.

The binary is called `auditor`. Run it in a terminal and you get a TUI; pipe it
and you get plain text, which is what you want in CI and git hooks.

![auditor finding SQL and command injection in a Python file](demo/demo.gif)

## Install

    cargo install code-auditor                # from crates.io

Prebuilt binaries for Linux, macOS and Windows are on the
[releases page](https://github.com/SpeedyCoder1192/code-auditor/releases), or:

    curl --proto '=https' --tlsv1.2 -LsSf https://github.com/SpeedyCoder1192/code-auditor/releases/latest/download/code-auditor-installer.sh | sh

Then install [Ollama](https://ollama.com) and run `auditor`. The default model
(about 4.7 GB) is downloaded on first use. On a machine without a GPU, set
`local_model = "qwen2.5-coder:1.5b"` for a faster but much less accurate model.

To build from source: `cargo build --release` (Rust 1.80 or newer). The binary
ends up in `target/release/auditor`.

## Use

    auditor scan src/                    # TUI on a terminal, plain text when piped
    auditor scan src/ --cli -v           # plain text, with details
    auditor scan src/ --format json      # JSON array on stdout
    auditor scan . --fail-on high        # exit 1 on high or critical findings
    auditor scan a.py b.py               # several files or directories
    auditor                              # TUI on the current directory

Exit codes: `0` clean, `1` findings at or above `--fail-on` (default `critical`),
`2` error. Findings go to stdout; errors and the one-line summary go to stderr.

TUI keys: `Q` quit, `P` apply patch for the selected file (original saved as
`<file>.orig`), `R` rescan selected file, `Shift+R` rescan all, `Tab` change
pane, arrows / `j` `k` / PageUp / PageDown scroll, `N` / `Shift+N` jump to next
or previous finding.

## CI and git hooks

GitHub Actions. Without `api-key` the action runs Ollama on the runner, which is
free but takes a few minutes:

    - uses: actions/checkout@v4
    - uses: SpeedyCoder1192/code-auditor@v1
      with:
        path: src/
        fail-on: high
        # api-key: ${{ secrets.OPENAI_API_KEY }}

[pre-commit](https://pre-commit.com):

    repos:
      - repo: https://github.com/SpeedyCoder1192/code-auditor
        rev: v1.0.1
        hooks:
          - id: auditor

Plain git hook (`.git/hooks/pre-commit`):

    #!/bin/sh
    files=$(git diff --cached --name-only --diff-filter=AM)
    [ -z "$files" ] && exit 0
    exec auditor scan $files --cli --fail-on high

## Engines

With `AUDITOR_API_KEY` set, requests go to the cloud:

- OpenAI-style API by default (`gpt-4o-mini`), or any compatible server via `AUDITOR_BASE_URL`
- Gemini when `AUDITOR_PROVIDER=gemini` or the model name starts with `gemini`

Without a key, requests go to a local server:

- Ollama at `http://127.0.0.1:11434`, model `qwen2.5-coder:7b`, pulled on first use
- llama.cpp's `llama-server` with `local_backend = "llamacpp"`

`--local` forces local even when a key is set.

## Config

Environment variables win over the file at `~/.config/code-auditor/config.toml`
(or `$XDG_CONFIG_HOME/code-auditor/config.toml`).

    provider       = "auto"                  # auto | local | openai | gemini
    cloud_model    = "gpt-4o-mini"
    local_backend  = "ollama"                # ollama | llamacpp
    local_url      = "http://127.0.0.1:11434"
    local_model    = "qwen2.5-coder:7b"      # or e.g. "llama3:8b-instruct-q4_0"
    auto_pull      = true
    chunk_lines    = 300
    max_file_bytes = 524288
    concurrency    = 2
    timeout_secs   = 300

    [ads]
    enabled = true
    url     = "https://raw.githubusercontent.com/SpeedyCoder1192/code-auditor/main/ad.txt"

Variables: `AUDITOR_API_KEY`, `AUDITOR_PROVIDER`, `AUDITOR_MODEL`,
`AUDITOR_BASE_URL`, `AUDITOR_LOCAL_URL`, `AUDITOR_LOCAL_MODEL`, `AUDITOR_AD_URL`,
`DISABLE_ADS`.

Keep the API key in the environment rather than the file if the file is
shared or checked in.

## Footer ad

The TUI footer shows one line of text, read from
[`ad.txt`](ad.txt) in this repo. It's a plain GET to raw.githubusercontent.com
with no identifiers or cookies (GitHub sees your IP, as with any request). The
line is cached for 30 minutes under `~/.cache/code-auditor/ad.json`, and the
CLI never fetches or prints it. Set `DISABLE_ADS=1` or `ads.enabled = false` to
turn it off, or point `ads.url` somewhere else.

Want your line there? Open an issue titled "Sponsor". Payment is in USDC
(Polygon or Ethereum), ETH or POL.

## Layout

    src/main.rs        argument parsing, picks CLI or TUI
    src/cli.rs         plain output, exit codes
    src/tui/           panes, key handling, highlighting, markdown rendering
    src/engine/        Engine trait; Ollama, OpenAI-style, Gemini
    src/ads.rs         footer ad fetch and cache
    src/model.rs       findings, patch application
    src/scan.rs        file discovery
    src/config.rs      config file and environment
    action.yml         GitHub Action
    demo/              vhs script for the demo GIF

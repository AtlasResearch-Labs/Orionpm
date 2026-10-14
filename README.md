# Orion — Process Manager

Built by Atlas Research Labs. Built for the World.

Orion is a production-grade process manager written in Rust.
It replaces PM2 entirely — including the features PM2 gates behind its paid subscription.

---

## Features

- Starts, stops, restarts, and monitors any process: Node.js, Python, Go, Bun, Deno, Java, or any binary
- Automatically restarts crashed processes with configurable exponential backoff
- Captures and stores stdout/stderr per process with a ring-buffer log engine
- Collects real CPU and memory metrics per managed PID via OS syscalls
- Sends alerts on crash, high memory, or high CPU via webhooks or email — no cloud dependency
- Generates startup scripts for systemd (Linux), launchd (macOS), and Windows Service Manager
- Ships with zero external runtime dependencies

---

## Installation

### Via NPM (Recommended)

Install globally:

```bash
npm install -g orionpm
```

Installs both **`orion`** and the 3-letter shortcut **`opm`** globally (`opm list`, `opm start`, `opm stop`).
Zero external download dependencies. Precompiled native binaries are bundled directly with the npm package for instant installation.

Execute without installation using `npx`:

```bash
npx orionpm start ecosystem.config.js
```

Or add as a project dev dependency:

```bash
npm install --save-dev orionpm
```

---

### Standalone Shell Installer

Linux & macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/AtlasResearch-Labs/Orionpm/main/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/AtlasResearch-Labs/Orionpm/main/install.ps1 | iex
```

---

### Build From Source (Rust 1.75+)

```bash
git clone https://github.com/AtlasResearch-Labs/Orionpm.git
cd Orionpm
cargo build --release
```

The compiled binaries will be at:
- `target/release/orion` — the CLI
- `target/release/oriond` — the daemon

---

## Quick Start

```bash
# Start the daemon (auto-started by orion commands if not running)
orion daemon start

# Start a process
orion start app.js --name my-app

# Start with file watching and auto-restart on edit
orion start app.js --name my-app --watch

# Start directly from a PM2 ecosystem file (drop-in compatibility)
orion start ecosystem.config.js
orion start ecosystem.json

# Import PM2 configuration to native orion.toml
orion import ecosystem.config.js -o orion.toml

# Or using a native config file
orion start orion.toml

# Check status
orion list

# Watch logs
orion logs my-app --follow

# Open the terminal monitor
orion monit

# Stop a process
orion stop my-app

# Install oriond as a system service (auto-start on boot)
orion startup
```

---

## Configuration

Create an `orion.toml` in your project directory:

```toml
[app.api-server]
command = "node"
args = ["dist/server.js"]
cwd = "/srv/api"
instances = 4
mode = "cluster"
restart_policy = "always"
max_restarts = 10
max_memory_mb = 512

[app.api-server.env]
NODE_ENV = "production"
PORT = "3000"

[app.api-server.alerts]
on_crash_webhook = "https://hooks.slack.com/services/..."
on_crash_email = "ops@yourteam.com"

[smtp]
host = "smtp.yourprovider.com"
port = 587
username = "alerts@yourteam.com"
password_env = "ORION_SMTP_PASSWORD"
from = "Orion Alerts <alerts@yourteam.com>"
tls = true
```

---

## CLI Reference

```
orion start <config|script> [options]
orion stop <name|id|all>
orion restart <name|id|all>
orion reload <name|id|all>
orion delete <name|id|all>
orion list (aliases: ls, status)
orion info <name|id>
orion logs <name|id> [--lines N] [--follow]
orion monit
orion save
orion restore
orion startup [--delete]
orion env <name|id>
orion daemon <status|start|kill>
orion import <file> [-o output.toml]
```

---

## Architecture

Orion consists of two binaries:

- `oriond` — the background daemon. Manages child processes, collects metrics, writes logs to SQLite, dispatches alerts.
- `orion` — the CLI. Connects to oriond over a local socket (Unix Domain Socket on Linux/macOS, Named Pipe on Windows) and sends NDJSON commands.

All communication between the CLI and daemon uses a newline-delimited JSON protocol over a local socket.
No network ports are opened. No external service is contacted unless you configure a webhook or SMTP alert.

---

## License

MIT

---

## Built By

Atlas Research Labs
dev@atlasresearchlabs.online

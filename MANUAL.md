# Orion Process Manager — Complete Manual & CLI Reference

**Atlas Research Labs** | Contact: `dev@atlasresearchlabs.online`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Installation & Requirements](#2-installation--requirements)
3. [Terminal Help Commands](#3-terminal-help-commands)
4. [Complete Command Reference](#4-complete-command-reference)
   - [start](#orion-start)
   - [stop](#orion-stop)
   - [restart](#orion-restart)
   - [reload](#orion-reload)
   - [delete](#orion-delete)
   - [list / ls / status](#orion-list--ls--status)
   - [info](#orion-info)
   - [logs](#orion-logs)
   - [monit](#orion-monit)
   - [save](#orion-save)
   - [restore](#orion-restore)
   - [startup](#orion-startup)
   - [env](#orion-env)
   - [daemon](#orion-daemon)
   - [import](#orion-import)
5. [Configuration File (`orion.toml`)](#5-configuration-file-oriontoml)
6. [PM2 Ecosystem Compatibility & Migration](#6-pm2-ecosystem-compatibility--migration)
7. [System Architecture & File Paths](#7-system-architecture--file-paths)
8. [Troubleshooting & Common Operations](#8-troubleshooting--common-operations)

---

## 1. Overview

Orion is an industrial-grade process supervisor written in Rust. It serves as a direct, high-performance replacement for PM2.

### Core Highlights
- **Dual Command Names**: Use either `orion` or the 3-letter shortcut **`opm`** (`opm list`, `opm stop`, `opm logs`).
- **Single Native Binary**: Sub-second boot times, zero V8 engine startup penalty.
- **Low Memory Overhead**: Baseline footprint of ~15 MB RSS compared to PM2's 80–150 MB.
- **Drop-in PM2 Compatibility**: Directly parses `ecosystem.config.js` and `ecosystem.json` files.
- **Embedded Logging & Metrics**: SQLite WAL-backed ring buffer log engine and OS-level CPU/memory sampling.
- **100% Free & Open**: Zero paywalls, zero cloud upsells, zero telemetry.

---

## 2. Installation & Requirements

### NPM Global Installation (Recommended)

```bash
npm install -g orionpm
```

This installs precompiled native binaries bundled directly inside the npm package. It registers both the **`orion`** and **`opm`** commands globally on your PATH.

### Shell One-Liner

**Linux & macOS:**
```bash
curl -fsSL https://raw.githubusercontent.com/AtlasResearch-Labs/Orionpm/main/install.sh | bash
```

**Windows (PowerShell):**
```powershell
irm https://raw.githubusercontent.com/AtlasResearch-Labs/Orionpm/main/install.ps1 | iex
```

### Verification

Check version with any of the following:

```bash
orion -v
# or using opm shortcut:
opm -v
# or full flag:
orion --version
```

Expected output:
```text
orion 0.1.1
```

---

## 3. Terminal Help Commands

Orion has a built-in help system accessible directly from the command line:

### Top-Level Help
Displays all available subcommands, global options, and version info:
```bash
orion --help
# or short form:
orion -h
```

### Subcommand Help
Displays the usage syntax, required arguments, and flags for any specific command:
```bash
orion <command> --help
# Examples:
orion start --help
orion logs --help
orion startup --help
orion import --help
orion daemon --help
```

### Alternative Help Syntax
```bash
orion help <command>
# Example:
orion help start
```

---

## 4. Complete Command Reference

### `orion start`
Starts a process from a script, executable, native configuration file (`orion.toml`), or PM2 ecosystem file.

```bash
orion start [OPTIONS] <TARGET>
```

#### Arguments
- `<TARGET>`: Path to a script (`app.js`, `server.py`, `binary`), an `orion.toml` config, or a PM2 ecosystem file (`ecosystem.config.js`, `ecosystem.json`).

#### Options
| Option | Description |
|---|---|
| `--name <NAME>` | Name of the process. Required when starting a single script directly. |
| `-i, --instances <N>` | Number of worker instances to spawn (`0` or `max` for all CPU cores). |
| `--watch` | Enable file watching; restarts process on changes in the working directory. |
| `--watch-path <PATH>` | Custom directory or file to watch (can be repeated). |
| `--max-memory <LIMIT>` | Memory restart threshold (e.g., `500M`, `1G`, `256MB`). |
| `--no-restart` | Disable automatic restarts if the process exits or crashes. |
| `--env <KEY=VALUE>` | Inject environment variables (repeatable: `--env PORT=3000 --env NODE_ENV=production`). |
| `--cwd <PATH>` | Set working directory for the process. |
| `--out <PATH>` | Mirror stdout stream to a specific file. |
| `--error <PATH>` | Mirror stderr stream to a specific file. |

#### Examples
```bash
# Start a simple Node.js application
orion start app.js --name my-api

# Start with cluster mode across all CPU cores
orion start app.js --name my-api -i max

# Start with live reload on file edit
orion start server.py --name backend --watch

# Start with memory limit and environment variables
orion start index.js --name worker --max-memory 500M --env NODE_ENV=production

# Start from PM2 ecosystem file
orion start ecosystem.config.js

# Start from native Orion configuration
orion start orion.toml
```

---

### `orion stop`
Stops a running process by sending SIGTERM followed by SIGKILL if it does not exit within the timeout window (default: 5000ms).

```bash
orion stop <TARGET>
```

#### Arguments
- `<TARGET>`: Process name, numeric ID, or `all` to stop all processes.

#### Examples
```bash
orion stop my-api
orion stop 0
orion stop all
```

---

### `orion restart` (Alias: `res`)
Restarts a process by performing a sequential stop and start.

```bash
orion restart <TARGET>
orion res <TARGET>
```

#### Arguments
- `<TARGET>`: Process name, numeric ID, or `all`.

#### Examples
```bash
orion restart my-api
orion res all
```

---

### `orion reload` (Alias: `gracefulReload`)
Performs a zero-downtime rolling reload. In cluster mode, workers are replaced one by one without dropping incoming connections.

```bash
orion reload <TARGET>
orion gracefulReload <TARGET>
```

#### Arguments
- `<TARGET>`: Process name, numeric ID, or `all`.

#### Examples
```bash
orion reload my-api
orion reload all
```

---

### `orion delete` (Aliases: `del`, `rm`)
Stops a process (if running) and removes it from Orion's registry and database.

```bash
orion delete <TARGET>
orion del <TARGET>
orion rm <TARGET>
```

#### Arguments
- `<TARGET>`: Process name, numeric ID, or `all`.

#### Examples
```bash
orion delete my-api
orion rm 0
orion del all
```

---

### `orion list` (Aliases: `ls`, `status`)
Displays a formatted status table of all managed processes.

```bash
orion list
orion ls
orion status
```

#### Output Columns
- **ID**: Unique numeric process index.
- **NAME**: Process name identifier.
- **STATUS**: Current state (`online`, `stopped`, `errored`, `stopping`).
- **MODE**: Execution mode (`fork` or `cluster`).
- **CPU**: Real-time CPU utilization percentage.
- **MEM**: Resident Set Size (RSS) memory consumption (e.g., `42.5 MB`).
- **UPTIME**: Human-readable uptime since last start.
- **RESTARTS**: Count of crashes or restarts.
- **COMMAND**: Executable command invoked.

---

### `orion info`
Shows detailed runtime metadata, file paths, and resource usage for a single process.

```bash
orion info <TARGET>
```

#### Arguments
- `<TARGET>`: Process name or numeric ID.

#### Example
```bash
orion info my-api
```

---

### `orion logs`
Streams or displays recent log lines from Orion's internal ring buffer.

```bash
orion logs [OPTIONS] <TARGET>
```

#### Arguments
- `<TARGET>`: Process name, numeric ID, or `all`.

#### Options
| Option | Description |
|---|---|
| `-n, --lines <N>` | Number of lines to show [default: 50]. |
| `-f, --follow` | Stream new incoming log entries in real-time. |

#### Examples
```bash
# View last 50 lines for my-api
orion logs my-api

# Follow logs in real time
orion logs my-api -f

# Follow combined logs of all processes
orion logs all -f --lines 100
```

---

### `orion monit`
Launches the interactive full-terminal OLED monitor.

```bash
orion monit
```

#### Features
- Real-time CPU and memory graphs per process.
- Live scrolling stdout/stderr log inspection.
- Process status and restart metrics.
- Navigation via Arrow keys (`Up` / `Down` to select process).
- Exit via `q` or `Ctrl+C`.

---

### `orion save`
Persists the current list of active processes and their configurations to disk.

```bash
orion save
```

This configuration is saved to the local database and used by `orion restore` and system boot scripts.

---

### `orion restore`
Restores and restarts all processes previously persisted with `orion save`.

```bash
orion restore
```

---

### `orion startup`
Configures Orion's daemon (`oriond`) to run automatically on system boot.

```bash
orion startup [OPTIONS]
```

#### Platform Implementation
- **Linux**: Generates and enables a systemd user or system service (`/etc/systemd/system/orion.service`).
- **macOS**: Registers a Launchd agent (`~/Library/LaunchAgents/online.atlasresearchlabs.orion.plist`).
- **Windows**: Registers a Windows Service via Service Manager.

#### Options
| Option | Description |
|---|---|
| `--delete` | Uninstalls and removes the boot service. |

#### Examples
```bash
# Enable auto-start on boot
sudo orion startup

# Disable auto-start
sudo orion startup --delete
```

---

### `orion env`
Displays all environment variables injected into a specific managed process.

```bash
orion env <TARGET>
```

#### Arguments
- `<TARGET>`: Process name or numeric ID.

---

### `orion daemon`
Directly controls the background supervisor daemon (`oriond`).

```bash
orion daemon <COMMAND>
```

#### Commands
| Subcommand | Description |
|---|---|
| `status` | Checks if `oriond` is active and responsive over IPC. |
| `start` | Launches `oriond` in the background if not already running. |
| `kill` | Gracefully shuts down `oriond` and stops all child processes. |

#### Examples
```bash
orion daemon status
orion daemon kill
orion daemon start
```

---

### `orion import`
Imports and converts existing PM2 configurations (`ecosystem.config.js`, `ecosystem.json`, `pm2.json`) into native `orion.toml` files.

```bash
orion import [OPTIONS] <SOURCE>
```

#### Arguments
- `<SOURCE>`: Path to the PM2 configuration file.

#### Options
| Option | Description |
|---|---|
| `-o, --output <PATH>` | Output path for the converted file [default: `orion.toml`]. |
| `--start` | Immediately starts all applications defined in the converted configuration. |

#### Examples
```bash
# Convert ecosystem.config.js to orion.toml
orion import ecosystem.config.js

# Convert and start immediately
orion import ecosystem.config.js -o production.toml --start
```

---

## 5. Configuration File (`orion.toml`)

Orion supports declarative configuration using TOML. Place an `orion.toml` file in your repository:

```toml
[app.api-service]
command = "node"
args = ["dist/server.js"]
cwd = "/var/www/api"
instances = 4
mode = "cluster"                  # "fork" or "cluster"
restart_policy = "always"         # "always", "on-failure", "never"
restart_delay_ms = 1000
max_restarts = 10
restart_window_secs = 60
max_memory_mb = 512
max_cpu_pct = 90.0

[app.api-service.env]
NODE_ENV = "production"
PORT = "3000"

[app.api-service.alerts]
on_crash_webhook = "https://hooks.slack.com/services/T00/B00/X00"
on_crash_email = "alerts@yourteam.com"

[smtp]
host = "smtp.mailgun.org"
port = 587
username = "postmaster@yourdomain.com"
password_env = "ORION_SMTP_PASSWORD"
from = "Orion Alerts <alerts@yourdomain.com>"
tls = true
```

### Starting from Config
```bash
orion start orion.toml
```

---

## 6. PM2 Ecosystem Compatibility & Migration

Orion natively reads PM2 configuration formats. You do not have to rewrite your files to start using Orion:

### Directly Start PM2 Ecosystem
```bash
orion start ecosystem.config.js
orion start ecosystem.json
```

### Supported PM2 Attributes
| PM2 Attribute | Orion Mapping |
|---|---|
| `name` | Process name |
| `script` | Command / script path |
| `args` | Command line arguments |
| `cwd` | Working directory |
| `instances` | Worker count (`max` or integer) |
| `exec_mode` | `cluster` / `fork` |
| `watch` | File watcher toggle |
| `max_memory_restart` | Memory threshold |
| `env` | Injected environment key-values |
| `out_file` | Stdout log mirror |
| `error_file` | Stderr log mirror |

---

## 7. System Architecture & File Paths

### Architecture
- **`oriond`**: Daemon process running as a background service. Manages process trees, handles POSIX signals, captures I/O streams, and logs to SQLite WAL.
- **`orion`**: Lightweight CLI client. Connects to `oriond` over a high-speed local socket (Unix Domain Socket on Linux/macOS, Named Pipe on Windows). Communication is non-blocking NDJSON.

### File Locations
| Resource | Linux / macOS | Windows |
|---|---|---|
| Database (`orion.db`) | `~/.local/share/orion/orion.db` | `%APPDATA%\orion\orion.db` |
| IPC Socket | `/tmp/orion.sock` or `~/.orion/orion.sock` | `\\.\pipe\orion-ipc` |
| CLI Binaries | `/usr/bin/orion` or `~/.orion/bin/` | `%USERPROFILE%\.orion\bin\` |

---

## 8. Troubleshooting & Common Operations

### Daemon Connection Issues
If the CLI reports `Cannot connect to oriond`:
```bash
# Check if oriond is running
orion daemon status

# Start daemon manually if needed
orion daemon start
```

### Viewing Daemon Output Logs
On Linux/macOS:
```bash
ORION_LOG=debug oriond
```

### Permission Errors with Node.js Global Binaries
If `npm install -g orionpm` encounters permission warnings on Linux:
```bash
sudo npm install -g orionpm
```
Orion's binary wrapper includes automatic permission escalation handling and will execute cleanly under standard user accounts.

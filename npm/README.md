# orionpm

Industrial-grade process manager written in Rust. Designed to replace PM2.
Built by Atlas Research Labs. Built for the World.

---

## Installation

### Global Installation

```bash
npm install -g orionpm
```

Installs both **`orion`** and the 3-letter shortcut **`opm`** globally.
Zero external download dependencies. Precompiled native binaries are bundled directly with the npm package for instant installation.

### Direct Execution with npx

```bash
npx orionpm start app.js
# or:
npx opm start app.js
```

### Local Project Dependency

```bash
npm install --save-dev orionpm
```

Add to `package.json`:

```json
{
  "scripts": {
    "start": "orion start ecosystem.config.js",
    "stop": "orion stop all",
    "status": "orion list"
  }
}
```

---

## Quick Start

```bash
# Start a process
orion start app.js --name my-app

# Start with live file watching and auto-restart on changes
orion start app.js --name my-app --watch

# Start directly from PM2 ecosystem file (drop-in replacement)
orion start ecosystem.config.js
orion start ecosystem.json

# Import PM2 configuration to native orion.toml
orion import ecosystem.config.js -o orion.toml

# Check status
orion list

# View logs
orion logs my-app --follow

# Open terminal monitor
orion monit

# Stop process
orion stop my-app

# Kill daemon
orion daemon kill
```

---

## Why Orion over PM2?

- Single Rust Native Binary: No runtime dependencies, no V8 startup penalty, sub-second boot time.
- Zero Memory Bloat: 15 MB RSS vs PM2's 80-150 MB baseline.
- Drop-in PM2 Compatibility: Directly parses ecosystem.config.js and ecosystem.json.
- 100% Free and Open Source: Zero paywalls, zero cloud upsells, zero telemetry.
- OLED Black UI: Clean #000000 monochrome terminal monitor.

---

## License

MIT - Atlas Research Labs <dev@atlasresearchlabs.online>

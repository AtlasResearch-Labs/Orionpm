#!/usr/bin/env node

/**
 * orion-runtime: Drop-in replacement for pm2-runtime in Docker / Kubernetes containers.
 * Runs directly in the foreground as PID 1, capturing container signals and streaming logs.
 */

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');
const { ensureBinary } = require('../lib/installer');

const args = process.argv.slice(2);
if (args.length === 1 && (args[0] === '-v' || args[0] === '-V' || args[0] === '--version')) {
  console.log(`orion-runtime ${require('../package.json').version}`);
  process.exit(0);
}

let binaryPath = ensureBinary('orion');

if (!binaryPath) {
  console.error('[orion-runtime] Native binary not found.');
  console.error('[orion-runtime] Supported platforms: linux-x64, win32-x64.');
  process.exit(1);
}

// Ensure execution permissions on Unix
if (process.platform !== 'win32') {
  try {
    fs.accessSync(binaryPath, fs.constants.X_OK);
  } catch {
    try {
      fs.chmodSync(binaryPath, 0o755);
    } catch {
      try {
        const homeDir = process.env.HOME || process.env.USERPROFILE || '';
        const userBinDir = path.join(homeDir, '.orion', 'bin');
        fs.mkdirSync(userBinDir, { recursive: true });
        const target = path.join(userBinDir, path.basename(binaryPath));
        if (!fs.existsSync(target) || fs.statSync(target).size !== fs.statSync(binaryPath).size) {
          fs.copyFileSync(binaryPath, target);
          fs.chmodSync(target, 0o755);
        }
        binaryPath = target;
      } catch {
        // Fall back to original path
      }
    }
  }
}

// Forward to orion runtime <args>
const child = spawn(binaryPath, ['runtime', ...process.argv.slice(2)], {
  stdio: 'inherit',
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code || 0);
  }
});


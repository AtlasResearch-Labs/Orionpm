#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');
const { ensureBinary } = require('../lib/installer');

let binaryPath = ensureBinary('oriond');

if (!binaryPath) {
  console.error('[oriond] Native binary not found.');
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
      // If installed by root into /usr/lib and we cannot chmod in place,
      // copy to user-writable ~/.orion/bin and chmod there.
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

const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: 'inherit',
  windowsHide: false,
});

child.on('error', (err) => {
  console.error('[oriond] Failed to start native daemon process:', err.message);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 0);
  }
});

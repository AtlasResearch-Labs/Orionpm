#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');
const { ensureBinary } = require('../lib/installer');

const args = process.argv.slice(2);
if (args.length === 1 && (args[0] === '-v' || args[0] === '-V' || args[0] === '--version')) {
  console.log(`orion ${require('../package.json').version}`);
  process.exit(0);
}

let binaryPath = ensureBinary('orion');

if (!binaryPath) {
  console.error('[orion] Native binary not found.');
  console.error('[orion] Supported platforms: linux-x64, win32-x64.');
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
  console.error('[orion] Failed to start native process:', err.message);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 0);
  }
});

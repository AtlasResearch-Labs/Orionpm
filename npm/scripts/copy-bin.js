const fs = require('fs');
const path = require('path');

const isWin = process.platform === 'win32';
const files = isWin ? ['orion.exe', 'oriond.exe'] : ['orion', 'oriond'];

const targetRelease = path.resolve(__dirname, '..', '..', 'target', 'release');
const targetDebug = path.resolve(__dirname, '..', '..', 'target', 'debug');
const nativeDir = path.resolve(__dirname, '..', 'bin', 'native');

if (!fs.existsSync(nativeDir)) {
  fs.mkdirSync(nativeDir, { recursive: true });
}

const sourceDir = fs.existsSync(path.join(targetRelease, files[0]))
  ? targetRelease
  : fs.existsSync(path.join(targetDebug, files[0]))
  ? targetDebug
  : null;

if (!sourceDir) {
  console.error('[orion] No compiled binaries found in target/release or target/debug.');
  console.error('[orion] Run "cargo build" or "cargo build --release" first.');
  process.exit(1);
}

for (const file of files) {
  const src = path.join(sourceDir, file);
  const dst = path.join(nativeDir, file);
  if (fs.existsSync(src)) {
    fs.copyFileSync(src, dst);
    if (!isWin) {
      fs.chmodSync(dst, 0o755);
    }
    console.log(`[orion] Copied ${file} -> bin/native/${file}`);
  } else {
    console.warn(`[orion] Warning: Source file ${src} not found.`);
  }
}

console.log('[orion] Done copying native binaries.');

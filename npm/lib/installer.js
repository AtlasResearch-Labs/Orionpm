const fs = require('fs');
const path = require('path');

function getPlatformKey() {
  return `${process.platform}-${process.arch}`;
}

function getBinaryName(base) {
  return process.platform === 'win32' ? `${base}.exe` : base;
}

function resolveExistingBinary(base) {
  const binaryName = getBinaryName(base);

  // 1. Packaged native binary in npm/bin/native/
  const localNative = path.join(__dirname, '..', 'bin', 'native', binaryName);
  if (fs.existsSync(localNative)) {
    return localNative;
  }

  // 2. Direct package bin directory
  const directBin = path.join(__dirname, '..', 'bin', binaryName);
  if (fs.existsSync(directBin)) {
    return directBin;
  }

  // 3. Platform optional package if installed
  try {
    const pkgName = `orionpm-${process.platform}-${process.arch}`;
    const pkgDir = path.dirname(require.resolve(`${pkgName}/package.json`));
    const platformCandidate = path.join(pkgDir, 'bin', binaryName);
    if (fs.existsSync(platformCandidate)) {
      return platformCandidate;
    }
  } catch {
    // Platform package not present
  }

  // 4. Workspace development build: target/release or target/debug
  const workspaceRelease = path.resolve(__dirname, '..', '..', 'target', 'release', binaryName);
  if (fs.existsSync(workspaceRelease)) {
    return workspaceRelease;
  }

  const workspaceDebug = path.resolve(__dirname, '..', '..', 'target', 'debug', binaryName);
  if (fs.existsSync(workspaceDebug)) {
    return workspaceDebug;
  }

  // 5. User home cache: ~/.orion/bin
  const homeDir = process.env.HOME || process.env.USERPROFILE || '';
  const userBin = path.join(homeDir, '.orion', 'bin', binaryName);
  if (fs.existsSync(userBin)) {
    return userBin;
  }

  return null;
}

function ensureBinary(base) {
  return resolveExistingBinary(base);
}

module.exports = {
  getPlatformKey,
  getBinaryName,
  resolveExistingBinary,
  ensureBinary,
};

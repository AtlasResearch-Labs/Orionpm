const path = require('path');
const fs = require('fs');

function getBinaryName(base) {
  return process.platform === 'win32' ? `${base}.exe` : base;
}

function resolveBinary(base) {
  const binaryName = getBinaryName(base);

  const localNative = path.join(__dirname, 'bin', 'native', binaryName);
  if (fs.existsSync(localNative)) {
    return localNative;
  }

  const directBin = path.join(__dirname, 'bin', binaryName);
  if (fs.existsSync(directBin)) {
    return directBin;
  }

  const workspaceDebug = path.resolve(__dirname, '..', 'target', 'debug', binaryName);
  if (fs.existsSync(workspaceDebug)) {
    return workspaceDebug;
  }

  const workspaceRelease = path.resolve(__dirname, '..', 'target', 'release', binaryName);
  if (fs.existsSync(workspaceRelease)) {
    return workspaceRelease;
  }

  const homeDir = process.env.HOME || process.env.USERPROFILE || '';
  const userBin = path.join(homeDir, '.orion', 'bin', binaryName);
  if (fs.existsSync(userBin)) {
    return userBin;
  }

  return null;
}

module.exports = {
  getOrionBinary: () => resolveBinary('orion'),
  getOriondBinary: () => resolveBinary('oriond'),
  // Aliases
  resolveBinary,
};

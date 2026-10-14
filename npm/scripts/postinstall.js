const fs = require('fs');
const path = require('path');
const https = require('https');
const { execSync } = require('child_process');

const pkg = require('../package.json');

const PLATFORM_MAP = {
  'win32-x64': {
    assetName: 'orion-windows-x86_64.zip',
    binaries: ['orion.exe', 'oriond.exe'],
  },
  'linux-x64': {
    assetName: 'orion-linux-x86_64.tar.gz',
    binaries: ['orion', 'oriond'],
  },
  'linux-arm64': {
    assetName: 'orion-linux-arm64.tar.gz',
    binaries: ['orion', 'oriond'],
  },
  'darwin-arm64': {
    assetName: 'orion-macos-arm64.tar.gz',
    binaries: ['orion', 'oriond'],
  },
  'darwin-x64': {
    assetName: 'orion-macos-x86_64.tar.gz',
    binaries: ['orion', 'oriond'],
  },
};

function getPlatformKey() {
  return `${process.platform}-${process.arch}`;
}

function followRedirects(url, callback) {
  const req = https.get(url, { timeout: 3500 }, (res) => {
    if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
      followRedirects(res.headers.location, callback);
    } else if (res.statusCode === 200) {
      callback(null, res);
    } else {
      callback(new Error(`HTTP ${res.statusCode} from ${url}`));
    }
  });
  req.on('timeout', () => {
    req.destroy(new Error('Download timed out'));
  });
  req.on('error', callback);
}

function install() {
  const key = getPlatformKey();
  const target = PLATFORM_MAP[key];

  if (!target) {
    console.warn(`[orion] Unsupported platform architecture: ${key}. You may need to compile from source via 'cargo build --release'.`);
    return;
  }

  let nativeDir = path.join(__dirname, '..', 'bin', 'native');
  try {
    if (!fs.existsSync(nativeDir)) {
      fs.mkdirSync(nativeDir, { recursive: true });
    }
    const testFile = path.join(nativeDir, '.perm_test');
    fs.writeFileSync(testFile, '');
    fs.unlinkSync(testFile);
  } catch {
    const homeDir = process.env.HOME || process.env.USERPROFILE || '';
    nativeDir = path.join(homeDir, '.orion', 'bin');
    if (!fs.existsSync(nativeDir)) {
      fs.mkdirSync(nativeDir, { recursive: true });
    }
  }

  // Check if binaries are already present
  const allExist = target.binaries.every((b) => fs.existsSync(path.join(nativeDir, b)));
  if (allExist) {
    return;
  }

  // Check if developer workspace binaries exist in target/debug or target/release
  const workspaceDebug = path.resolve(__dirname, '..', '..', 'target', 'debug');
  const workspaceRelease = path.resolve(__dirname, '..', '..', 'target', 'release');
  const sourceDir = fs.existsSync(path.join(workspaceRelease, target.binaries[0]))
    ? workspaceRelease
    : fs.existsSync(path.join(workspaceDebug, target.binaries[0]))
    ? workspaceDebug
    : null;

  if (sourceDir) {
    for (const b of target.binaries) {
      const src = path.join(sourceDir, b);
      const dst = path.join(nativeDir, b);
      if (fs.existsSync(src)) {
        fs.copyFileSync(src, dst);
        if (process.platform !== 'win32') {
          fs.chmodSync(dst, 0o755);
        }
      }
    }
    console.log(`[orion] Installed native binaries from local build: ${sourceDir}`);
    return;
  }

  const url = `https://github.com/AtlasResearch-Labs/Orionpm/releases/download/v${pkg.version}/${target.assetName}`;
  const archivePath = path.join(nativeDir, target.assetName);

  console.log(`[orion] Downloading prebuilt binary from ${url}...`);

  followRedirects(url, (err, stream) => {
    if (err) {
      console.warn(`[orion] Note: Prebuilt release asset not available yet (${err.message}).`);
      console.warn(`[orion] You can build locally with 'cargo build --release' or download from releases page.`);
      return;
    }

    const fileStream = fs.createWriteStream(archivePath);
    stream.pipe(fileStream);

    fileStream.on('finish', () => {
      fileStream.close(() => {
        try {
          console.log(`[orion] Extracting ${target.assetName}...`);
          execSync(`tar -xf "${archivePath}" -C "${nativeDir}"`, { stdio: 'ignore' });
          fs.unlinkSync(archivePath);

          for (const b of target.binaries) {
            const binPath = path.join(nativeDir, b);
            if (fs.existsSync(binPath) && process.platform !== 'win32') {
              fs.chmodSync(binPath, 0o755);
            }
          }
          console.log('[orion] Native binaries installed successfully.');
        } catch (extractErr) {
          console.warn(`[orion] Extraction warning: ${extractErr.message}`);
        }
      });
    });
  });
}

install();

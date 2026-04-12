const path = require("path");
const os = require("os");

const PLATFORMS = {
  "darwin-arm64": "@aquintanar/browser39-darwin-arm64",
  "darwin-x64": "@aquintanar/browser39-darwin-x64",
  "linux-x64": "@aquintanar/browser39-linux-x64",
  "linux-arm64": "@aquintanar/browser39-linux-arm64",
  "win32-x64": "@aquintanar/browser39-win32-x64",
};

function executablePath() {
  const key = `${os.platform()}-${os.arch()}`;
  const pkg = PLATFORMS[key];
  if (!pkg) {
    throw new Error(
      `browser39: unsupported platform ${key}. Supported: ${Object.keys(PLATFORMS).join(", ")}`
    );
  }

  const binName = os.platform() === "win32" ? "browser39.exe" : "browser39";
  try {
    const binDir = path.dirname(require.resolve(`${pkg}/package.json`));
    return path.join(binDir, "bin", binName);
  } catch {
    throw new Error(
      `browser39: platform package "${pkg}" not installed. Run: npm install ${pkg}`
    );
  }
}

module.exports = { executablePath };

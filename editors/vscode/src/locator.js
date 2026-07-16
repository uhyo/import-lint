"use strict";

// The extension's binary locator (docs/PLAN-lsp.md §4, decision E6). Plain
// CommonJS so it can be unit-tested with `node --test` without a build step,
// mirroring the npm shim's testing approach (npm/import-lint/test/shim.test.js).
//
// Reuses the shim's platform-key computation and platform-package resolution
// rather than reimplementing it — see npm/import-lint/bin/import-lint.js.

const path = require("node:path");
const { existsSync: fsExistsSync } = require("node:fs");
const { createRequire: nodeCreateRequire } = require("node:module");

const shim = require("../../../npm/import-lint/bin/import-lint.js");

const MIN_LSP_VERSION = "0.1.2";

/**
 * Locates the import-lint binary to use for the LSP server. Resolution order
 * (PLAN E6):
 *   1. `settingsBinaryPath` (importLint.binaryPath setting) — if set, must
 *      exist; no fall-through on a bad explicit setting.
 *   2. Workspace node_modules: `@import-lint/cli` -> platform binary package,
 *      resolved relative to the cli package's own location.
 *   3. PATH scan.
 *
 * All external inputs are injectable for testability.
 */
function locateBinary({
  settingsBinaryPath,
  workspaceRoot,
  platformKey = shim.computePlatformKey(),
  env = process.env,
  existsSync = fsExistsSync,
  createRequire = nodeCreateRequire,
} = {}) {
  // 1. Explicit setting.
  if (typeof settingsBinaryPath === "string" && settingsBinaryPath.length > 0) {
    if (existsSync(settingsBinaryPath)) {
      return { ok: true, path: settingsBinaryPath, source: "settings" };
    }
    return {
      ok: false,
      reason: "settings-path-missing",
      detail: settingsBinaryPath,
    };
  }

  // 2. Workspace node_modules.
  if (workspaceRoot) {
    const req = createRequire(path.join(workspaceRoot, "package.json"));
    let cliPackageJsonPath;
    try {
      cliPackageJsonPath = req.resolve("@import-lint/cli/package.json");
    } catch {
      cliPackageJsonPath = undefined;
    }

    if (cliPackageJsonPath) {
      const req2 = createRequire(cliPackageJsonPath);
      const binPath = shim.resolveBinaryPath({
        platformKey,
        env: {},
        requireResolve: req2.resolve,
      });
      if (binPath) {
        return { ok: true, path: binPath, source: "workspace" };
      }
      return {
        ok: false,
        reason: "platform-package-missing",
        detail: platformKey,
      };
    }
  }

  // 3. PATH scan.
  const pathEnv = env.PATH || "";
  const binaryName = shim.binaryFileName(platformKey);
  const dirs = pathEnv.length > 0 ? pathEnv.split(path.delimiter) : [];
  for (const dir of dirs) {
    if (!dir) {
      continue;
    }
    const candidate = path.join(dir, binaryName);
    if (existsSync(candidate)) {
      return { ok: true, path: candidate, source: "path" };
    }
  }

  return { ok: false, reason: "not-found" };
}

/** Parses `import-lint <semver>` output into just the semver, or null. */
function parseVersionOutput(stdout) {
  if (typeof stdout !== "string") {
    return null;
  }
  const match = stdout.trim().match(/^import-lint\s+(\d+\.\d+\.\d+)$/);
  return match ? match[1] : null;
}

/** Numeric dot-part comparison: is `version` >= `minimum`? */
function isVersionAtLeast(version, minimum) {
  const a = String(version).split(".").map(Number);
  const b = String(minimum).split(".").map(Number);
  const len = Math.max(a.length, b.length);
  for (let i = 0; i < len; i++) {
    const av = a[i] || 0;
    const bv = b[i] || 0;
    if (av !== bv) {
      return av > bv;
    }
  }
  return true;
}

module.exports = {
  locateBinary,
  parseVersionOutput,
  isVersionAtLeast,
  MIN_LSP_VERSION,
};

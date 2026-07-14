"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");
const fs = require("node:fs");
const os = require("node:os");

const locator = require("../src/locator.js");

/**
 * Builds a fake `createRequire` that maps a caller filename to a fake
 * `require.resolve` function, so workspace-resolution tests never touch the
 * real module resolution algorithm.
 */
function fakeCreateRequire(resolversByFilename) {
  return (filename) => {
    const resolve = resolversByFilename[filename];
    if (!resolve) {
      throw new Error(`no fake resolver registered for createRequire(${filename})`);
    }
    return { resolve };
  };
}

// ---------------------------------------------------------------------------
// locateBinary: settings path
// ---------------------------------------------------------------------------

test("locateBinary: settings path exists -> ok, source settings", () => {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "import-lint-locator-test-"));
  try {
    const binPath = path.join(tmpDir, "import-lint");
    fs.writeFileSync(binPath, "#!/bin/sh\n");

    const result = locator.locateBinary({ settingsBinaryPath: binPath });
    assert.deepEqual(result, { ok: true, path: binPath, source: "settings" });
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});

test("locateBinary: settings path missing -> settings-path-missing, no fall-through", () => {
  const result = locator.locateBinary({
    settingsBinaryPath: "/nonexistent/path/to/import-lint",
    workspaceRoot: "/some/workspace", // must NOT be consulted
    env: { PATH: "/also/should/not/be/consulted" },
    existsSync: () => false,
  });
  assert.deepEqual(result, {
    ok: false,
    reason: "settings-path-missing",
    detail: "/nonexistent/path/to/import-lint",
  });
});

// ---------------------------------------------------------------------------
// locateBinary: workspace node_modules
// ---------------------------------------------------------------------------

test("locateBinary: workspace resolution happy path", () => {
  const workspaceRoot = path.join("/workspace", "project");
  const workspacePkgJson = path.join(workspaceRoot, "package.json");
  const cliPkgJson = path.join(
    workspaceRoot,
    "node_modules",
    "@import-lint",
    "cli",
    "package.json",
  );
  const platformBin = path.join(
    workspaceRoot,
    "node_modules",
    "@import-lint",
    "linux-x64-gnu",
    "import-lint",
  );

  const createRequire = fakeCreateRequire({
    [workspacePkgJson]: (specifier) => {
      if (specifier === "@import-lint/cli/package.json") {
        return cliPkgJson;
      }
      throw new Error(`Cannot find module '${specifier}'`);
    },
    [cliPkgJson]: (specifier) => {
      if (specifier === "@import-lint/linux-x64-gnu/import-lint") {
        return platformBin;
      }
      throw new Error(`Cannot find module '${specifier}'`);
    },
  });

  const result = locator.locateBinary({
    workspaceRoot,
    platformKey: "linux-x64-gnu",
    createRequire,
  });

  assert.deepEqual(result, { ok: true, path: platformBin, source: "workspace" });
});

test("locateBinary: cli package found but platform package missing -> platform-package-missing", () => {
  const workspaceRoot = path.join("/workspace", "project");
  const workspacePkgJson = path.join(workspaceRoot, "package.json");
  const cliPkgJson = path.join(
    workspaceRoot,
    "node_modules",
    "@import-lint",
    "cli",
    "package.json",
  );

  const createRequire = fakeCreateRequire({
    [workspacePkgJson]: (specifier) => {
      if (specifier === "@import-lint/cli/package.json") {
        return cliPkgJson;
      }
      throw new Error(`Cannot find module '${specifier}'`);
    },
    [cliPkgJson]: () => {
      throw new Error("Cannot find module '@import-lint/linux-x64-gnu/import-lint'");
    },
  });

  const result = locator.locateBinary({
    workspaceRoot,
    platformKey: "linux-x64-gnu",
    createRequire,
  });

  assert.deepEqual(result, {
    ok: false,
    reason: "platform-package-missing",
    detail: "linux-x64-gnu",
  });
});

test("locateBinary: cli package itself not found in workspace -> falls through to PATH scan", () => {
  const workspaceRoot = path.join("/workspace", "project");
  const workspacePkgJson = path.join(workspaceRoot, "package.json");

  const createRequire = fakeCreateRequire({
    [workspacePkgJson]: () => {
      throw new Error("Cannot find module '@import-lint/cli/package.json'");
    },
  });

  const result = locator.locateBinary({
    workspaceRoot,
    platformKey: "linux-x64-gnu",
    createRequire,
    env: { PATH: "" },
    existsSync: () => false,
  });

  assert.deepEqual(result, { ok: false, reason: "not-found" });
});

// ---------------------------------------------------------------------------
// locateBinary: PATH scan
// ---------------------------------------------------------------------------

test("locateBinary: no workspaceRoot -> PATH scan hit", () => {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "import-lint-locator-test-"));
  try {
    const binPath = path.join(tmpDir, "import-lint");
    fs.writeFileSync(binPath, "#!/bin/sh\n");

    const result = locator.locateBinary({
      workspaceRoot: undefined,
      platformKey: "linux-x64-gnu",
      env: { PATH: [tmpDir, "/nonexistent/other/dir"].join(path.delimiter) },
    });

    assert.deepEqual(result, { ok: true, path: binPath, source: "path" });
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});

test("locateBinary: nothing anywhere -> not-found", () => {
  const result = locator.locateBinary({
    workspaceRoot: undefined,
    platformKey: "linux-x64-gnu",
    env: { PATH: undefined },
    existsSync: () => false,
  });
  assert.deepEqual(result, { ok: false, reason: "not-found" });
});

test("locateBinary: PATH scan tolerates undefined env.PATH", () => {
  const result = locator.locateBinary({
    platformKey: "linux-x64-gnu",
    env: {},
    existsSync: () => false,
  });
  assert.deepEqual(result, { ok: false, reason: "not-found" });
});

// ---------------------------------------------------------------------------
// parseVersionOutput
// ---------------------------------------------------------------------------

test("parseVersionOutput: well-formed output with trailing newline", () => {
  assert.equal(locator.parseVersionOutput("import-lint 0.1.2\n"), "0.1.2");
});

test("parseVersionOutput: garbage input -> null", () => {
  assert.equal(locator.parseVersionOutput("not a version string"), null);
});

test("parseVersionOutput: empty string -> null", () => {
  assert.equal(locator.parseVersionOutput(""), null);
});

test("parseVersionOutput: non-string input -> null", () => {
  assert.equal(locator.parseVersionOutput(null), null);
  assert.equal(locator.parseVersionOutput(undefined), null);
});

// ---------------------------------------------------------------------------
// isVersionAtLeast
// ---------------------------------------------------------------------------

test("isVersionAtLeast: equal versions -> true", () => {
  assert.equal(locator.isVersionAtLeast("0.1.2", "0.1.2"), true);
});

test("isVersionAtLeast: patch below minimum -> false", () => {
  assert.equal(locator.isVersionAtLeast("0.1.1", "0.1.2"), false);
});

test("isVersionAtLeast: minor above minimum -> true", () => {
  assert.equal(locator.isVersionAtLeast("0.2.0", "0.1.2"), true);
});

test("isVersionAtLeast: numeric (not lexicographic) comparison, 0.10.0 vs 0.9.9", () => {
  assert.equal(locator.isVersionAtLeast("0.10.0", "0.9.9"), true);
  assert.equal(locator.isVersionAtLeast("0.9.9", "0.10.0"), false);
});

test("MIN_LSP_VERSION is 0.1.2", () => {
  assert.equal(locator.MIN_LSP_VERSION, "0.1.2");
});

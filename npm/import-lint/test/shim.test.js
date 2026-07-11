"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");
const fs = require("node:fs");
const os = require("node:os");
const { spawnSync } = require("node:child_process");

const shim = require("../bin/import-lint.js");

const shimPath = path.join(__dirname, "..", "bin", "import-lint.js");

test("computePlatformKey: darwin-arm64", () => {
  assert.equal(shim.computePlatformKey({ platform: "darwin", arch: "arm64" }), "darwin-arm64");
});

test("computePlatformKey: darwin-x64", () => {
  assert.equal(shim.computePlatformKey({ platform: "darwin", arch: "x64" }), "darwin-x64");
});

test("computePlatformKey: win32-x64", () => {
  assert.equal(shim.computePlatformKey({ platform: "win32", arch: "x64" }), "win32-x64");
});

test("computePlatformKey: linux glibc, detected via process.report", () => {
  const key = shim.computePlatformKey({
    platform: "linux",
    arch: "x64",
    report: { getReport: () => ({ header: { glibcVersionRuntime: "2.31" } }) },
  });
  assert.equal(key, "linux-x64-gnu");
});

test("computePlatformKey: linux musl, detected via process.report (glibcVersionRuntime absent)", () => {
  const key = shim.computePlatformKey({
    platform: "linux",
    arch: "arm64",
    report: { getReport: () => ({ header: {} }) },
  });
  assert.equal(key, "linux-arm64-musl");
});

test("computePlatformKey: linux, process.report unavailable, /etc/alpine-release present -> musl", () => {
  // `report: null` (not `undefined`) to simulate "unavailable" — an explicit
  // `undefined` would fall through to the real `process.report` default.
  const key = shim.computePlatformKey({
    platform: "linux",
    arch: "x64",
    report: null,
    existsSync: (p) => p === "/etc/alpine-release",
  });
  assert.equal(key, "linux-x64-musl");
});

test("computePlatformKey: linux, process.report unavailable, /etc/alpine-release absent -> gnu", () => {
  const key = shim.computePlatformKey({
    platform: "linux",
    arch: "x64",
    report: null,
    existsSync: () => false,
  });
  assert.equal(key, "linux-x64-gnu");
});

test("binaryFileName: .exe suffix only for win32 keys", () => {
  assert.equal(shim.binaryFileName("win32-x64"), "import-lint.exe");
  assert.equal(shim.binaryFileName("linux-x64-gnu"), "import-lint");
  assert.equal(shim.binaryFileName("darwin-arm64"), "import-lint");
});

test("resolveBinaryPath: IMPORT_LINT_BINARY short-circuits package resolution", () => {
  const binPath = shim.resolveBinaryPath({
    platformKey: "linux-x64-gnu",
    env: { IMPORT_LINT_BINARY: "/some/custom/path" },
    requireResolve: () => {
      throw new Error("requireResolve should not be called when the env override is set");
    },
  });
  assert.equal(binPath, "/some/custom/path");
});

test("resolveBinaryPath: returns null (not a throw) when the platform package can't be resolved", () => {
  const binPath = shim.resolveBinaryPath({
    platformKey: "linux-x64-gnu",
    env: {},
    requireResolve: () => {
      throw new Error("Cannot find module");
    },
  });
  assert.equal(binPath, null);
});

test("resolveBinaryPath: resolves the platform-specific binary specifier", () => {
  let requestedSpecifier;
  const binPath = shim.resolveBinaryPath({
    platformKey: "win32-x64",
    env: {},
    requireResolve: (specifier) => {
      requestedSpecifier = specifier;
      return "/resolved/path/import-lint.exe";
    },
  });
  assert.equal(requestedSpecifier, "@import-lint/win32-x64/import-lint.exe");
  assert.equal(binPath, "/resolved/path/import-lint.exe");
});

test("unsupportedPlatformError: mentions the computed key, every supported target, and both fallbacks (P8)", () => {
  const message = shim.unsupportedPlatformError("freebsd-x64");
  assert.match(message, /freebsd-x64/);
  for (const target of shim.SUPPORTED_TARGETS) {
    assert.ok(message.includes(target), `expected error message to mention "${target}"`);
  }
  assert.match(message, /cargo install import-lint/);
  assert.match(message, /github\.com\/uhyo\/import-lint\/releases/);
});

test("integration: no matching platform package -> exit 2, stderr carries the P8 error", () => {
  const env = { ...process.env };
  delete env.IMPORT_LINT_BINARY;

  const result = spawnSync(process.execPath, [shimPath, "--version"], {
    encoding: "utf8",
    env,
  });

  assert.equal(result.status, 2);
  assert.match(result.stderr, /no prebuilt binary found for platform/);
  assert.match(result.stderr, /cargo install import-lint/);
});

test(
  "integration: IMPORT_LINT_BINARY override runs that binary and mirrors its exit code",
  { skip: process.platform === "win32" },
  () => {
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "import-lint-shim-test-"));
    try {
      const stubPath = path.join(tmpDir, "stub-bin");
      fs.writeFileSync(
        stubPath,
        `#!/usr/bin/env node\nprocess.stdout.write("stub ran with: " + process.argv.slice(2).join(" ") + "\\n");\nprocess.exit(7);\n`,
      );
      fs.chmodSync(stubPath, 0o755);

      const env = { ...process.env, IMPORT_LINT_BINARY: stubPath };
      const result = spawnSync(process.execPath, [shimPath, "--some-flag"], {
        encoding: "utf8",
        env,
      });

      assert.equal(result.status, 7);
      assert.match(result.stdout, /stub ran with: --some-flag/);
    } finally {
      fs.rmSync(tmpDir, { recursive: true, force: true });
    }
  },
);

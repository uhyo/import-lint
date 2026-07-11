#!/usr/bin/env node
"use strict";

// The launcher shim (docs/PLAN.md §3): locates the platform-specific binary
// package that `optionalDependencies` should have installed and execs it.
// Kept intentionally dumb — no update checks, no telemetry, no config — since
// it runs on every single lint invocation.

const fs = require("fs");
const { spawnSync } = require("child_process");

const SUPPORTED_TARGETS = [
  "darwin-arm64",
  "darwin-x64",
  "linux-x64-gnu",
  "linux-x64-musl",
  "linux-arm64-gnu",
  "win32-x64",
];

/**
 * `${process.platform}-${process.arch}`, plus a `-gnu`/`-musl` suffix on
 * Linux. All external inputs are injectable so this is testable without
 * mocking globals.
 */
function computePlatformKey({
  platform = process.platform,
  arch = process.arch,
  report = process.report,
  existsSync = fs.existsSync,
} = {}) {
  if (platform !== "linux") {
    return `${platform}-${arch}`;
  }

  let libc;
  if (report && typeof report.getReport === "function") {
    const glibcVersion = report.getReport()?.header?.glibcVersionRuntime;
    libc = glibcVersion ? "gnu" : "musl";
  } else {
    // `process.report` unavailable: fall back to an Alpine-Linux probe.
    libc = existsSync("/etc/alpine-release") ? "musl" : "gnu";
  }
  return `linux-${arch}-${libc}`;
}

function binaryFileName(platformKey) {
  return platformKey.startsWith("win32-") ? "import-lint.exe" : "import-lint";
}

/**
 * Resolves the absolute path to the real binary, honoring the
 * `IMPORT_LINT_BINARY` escape hatch. Returns `null` (not a throw) when the
 * platform package can't be resolved, so callers can render the P8 error.
 */
function resolveBinaryPath({
  platformKey,
  env = process.env,
  requireResolve = require.resolve,
} = {}) {
  if (env.IMPORT_LINT_BINARY) {
    return env.IMPORT_LINT_BINARY;
  }
  const specifier = `@import-lint/${platformKey}/${binaryFileName(platformKey)}`;
  try {
    return requireResolve(specifier);
  } catch {
    return null;
  }
}

/** The P8 actionable error: computed key, supported targets, fallbacks. */
function unsupportedPlatformError(platformKey) {
  return [
    `import-lint: no prebuilt binary found for platform "${platformKey}".`,
    "",
    "Supported platforms:",
    ...SUPPORTED_TARGETS.map((target) => `  - ${target}`),
    "",
    "This usually means:",
    "  - your platform isn't one of the ones listed above, or",
    "  - the matching @import-lint/<platform> optional dependency wasn't installed",
    "    (e.g. `npm install --omit=optional` / `--ignore-scripts`), or",
    "  - node_modules is corrupted; try reinstalling.",
    "",
    "Fallbacks:",
    "  - cargo install import-lint",
    "  - download a prebuilt binary: https://github.com/uhyo/import-lint/releases",
  ].join("\n");
}

function main(argv = process.argv.slice(2)) {
  const platformKey = computePlatformKey();
  const binPath = resolveBinaryPath({ platformKey });

  if (!binPath) {
    process.stderr.write(unsupportedPlatformError(platformKey) + "\n");
    process.exit(2);
    return;
  }

  const result = spawnSync(binPath, argv, { stdio: "inherit" });

  if (result.error) {
    process.stderr.write(
      `import-lint: failed to execute "${binPath}": ${result.error.message}\n`,
    );
    process.exit(2);
    return;
  }

  if (result.signal) {
    // Re-raise so the parent shell sees the same termination signal rather
    // than a plain exit code.
    process.kill(process.pid, result.signal);
    return;
  }

  process.exit(result.status === null ? 1 : result.status);
}

module.exports = {
  SUPPORTED_TARGETS,
  computePlatformKey,
  binaryFileName,
  resolveBinaryPath,
  unsupportedPlatformError,
  main,
};

if (require.main === module) {
  main();
}

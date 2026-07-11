#!/usr/bin/env node
"use strict";

// Local end-to-end smoke test (docs/PLAN.md §5): builds the real Rust binary,
// assembles just the host platform's npm package (into a *temp copy* of
// npm/, so the checked-in 0.0.0 package.jsons are never touched), packs the
// main + host platform packages, installs them into a throwaway project via
// npm `overrides` (so no registry fetch happens), then exercises the
// installed `import-lint` CLI through the shim: `--version` and a real lint
// of a two-file `@package`-violation fixture.
//
// This is also the entry point CI's `npm-smoke` job will call in N2, so
// local and CI runs can't drift (docs/PLAN.md §5).

import { execFileSync, spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { assemble } from "./assemble.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..", "..");
const npmRoot = path.resolve(__dirname, "..");
const require = createRequire(import.meta.url);

const DEV_VERSION = "0.0.0-smoke";

function log(step) {
  console.log(`\n=== smoke: ${step} ===`);
}

function main() {
  const { computePlatformKey, binaryFileName } = require(
    path.join(npmRoot, "import-lint", "bin", "import-lint.js"),
  );
  const hostKey = computePlatformKey();
  console.log(`smoke.mjs: host platform key = ${hostKey}`);

  const baseTmp = fs.mkdtempSync(path.join(os.tmpdir(), "import-lint-smoke-"));
  try {
    // 1. Build the real binary.
    log("cargo build --release");
    execFileSync("cargo", ["build", "--release", "-p", "import-lint"], {
      cwd: repoRoot,
      stdio: "inherit",
    });
    const builtBinaryName = hostKey.startsWith("win32-") ? "import-lint.exe" : "import-lint";
    const builtBinaryPath = path.join(repoRoot, "target", "release", builtBinaryName);
    if (!fs.existsSync(builtBinaryPath)) {
      throw new Error(`smoke.mjs: expected built binary at "${builtBinaryPath}", not found`);
    }

    // 2. Assemble just the host platform package, into a temp copy of npm/
    //    so the working tree stays clean.
    log("assemble (temp copy of npm/)");
    const tmpNpmRoot = path.join(baseTmp, "npm");
    fs.cpSync(npmRoot, tmpNpmRoot, { recursive: true });

    const distDir = path.join(baseTmp, "dist", hostKey);
    fs.mkdirSync(distDir, { recursive: true });
    fs.copyFileSync(builtBinaryPath, path.join(distDir, binaryFileName(hostKey)));

    const { assembled } = assemble({
      version: DEV_VERSION,
      dist: path.join(baseTmp, "dist"),
      only: hostKey,
      npmRoot: tmpNpmRoot,
    });
    console.log(`smoke.mjs: assembled ${assembled.join(", ")} at version ${DEV_VERSION}`);

    // 3. npm pack the main package and the host platform package.
    log("npm pack");
    const packsDir = path.join(baseTmp, "packs");
    fs.mkdirSync(packsDir, { recursive: true });
    const mainTarball = npmPack(path.join(tmpNpmRoot, "import-lint"), packsDir);
    const platformTarball = npmPack(path.join(tmpNpmRoot, "platform", hostKey), packsDir);
    console.log(`smoke.mjs: packed ${mainTarball}`);
    console.log(`smoke.mjs: packed ${platformTarball}`);

    // 4. Install into a throwaway project. `overrides` forces the scoped
    //    platform dependency to resolve to the local tarball instead of the
    //    registry, so this install never hits the network.
    log("npm install (temp project)");
    const projectDir = path.join(baseTmp, "project");
    fs.mkdirSync(projectDir, { recursive: true });
    fs.writeFileSync(
      path.join(projectDir, "package.json"),
      `${JSON.stringify(
        {
          name: "import-lint-smoke-project",
          version: "0.0.0",
          private: true,
          dependencies: {
            "import-lint": `file:${mainTarball}`,
          },
          overrides: {
            [`@import-lint/${hostKey}`]: `file:${platformTarball}`,
          },
        },
        null,
        2,
      )}\n`,
    );
    execFileSync("npm", ["install", "--no-audit", "--no-fund"], {
      cwd: projectDir,
      stdio: "inherit",
    });

    const shimPath = path.join(projectDir, "node_modules", "import-lint", "bin", "import-lint.js");
    if (!fs.existsSync(shimPath)) {
      throw new Error(`smoke.mjs: expected installed shim at "${shimPath}", not found`);
    }
    const installedBinaryPath = path.join(
      projectDir,
      "node_modules",
      "@import-lint",
      hostKey,
      binaryFileName(hostKey),
    );
    if (!fs.existsSync(installedBinaryPath)) {
      throw new Error(
        `smoke.mjs: expected installed platform binary at "${installedBinaryPath}", not found`,
      );
    }

    // 5. Exercise the shim: --version.
    //
    // Note: the CLI's --version comes from clap's built-in flag, which prints
    // the Rust crate's own compiled-in Cargo.toml version — NOT the npm dev
    // version stamped above (those two are only guaranteed to match on a
    // real release, where both come from the same tag). This step therefore
    // asserts the shim successfully resolved and executed the freshly built
    // host binary (exit 0, well-formed "import-lint x.y.z" output), not a
    // literal match against DEV_VERSION.
    log("run: import-lint --version");
    const versionResult = spawnSync(process.execPath, [shimPath, "--version"], {
      encoding: "utf8",
    });
    console.log(`stdout: ${versionResult.stdout.trim()}`);
    if (versionResult.status !== 0) {
      throw new Error(`smoke.mjs: --version exited ${versionResult.status}, expected 0`);
    }
    if (!/^import-lint \d+\.\d+\.\d+/.test(versionResult.stdout.trim())) {
      throw new Error(
        `smoke.mjs: --version output "${versionResult.stdout.trim()}" doesn't look like "import-lint x.y.z"`,
      );
    }

    // 6. Exercise the shim: lint a real `@package` violation fixture.
    log("run: import-lint (violation fixture)");
    const fixtureDir = path.join(baseTmp, "fixture");
    writeFixture(fixtureDir);
    const lintResult = spawnSync(process.execPath, [shimPath], {
      cwd: fixtureDir,
      encoding: "utf8",
    });
    console.log(lintResult.stdout);
    if (lintResult.status !== 1) {
      throw new Error(
        `smoke.mjs: lint of violation fixture exited ${lintResult.status}, expected 1\nstdout:\n${lintResult.stdout}\nstderr:\n${lintResult.stderr}`,
      );
    }
    if (!lintResult.stdout.includes("helper")) {
      throw new Error(
        `smoke.mjs: lint output doesn't mention the violated export "helper":\n${lintResult.stdout}`,
      );
    }

    log("PASS");
    console.log("smoke.mjs: all checks passed.");
  } finally {
    fs.rmSync(baseTmp, { recursive: true, force: true });
  }
}

function npmPack(pkgDir, destDir) {
  const output = execFileSync(
    "npm",
    ["pack", pkgDir, "--pack-destination", destDir, "--json"],
    { encoding: "utf8" },
  );
  const [{ filename }] = JSON.parse(output);
  return path.join(destDir, filename);
}

/**
 * A single `@package` violation: `src/consumer.ts` imports a `@package`
 * export from the sibling `src/internal/util.ts` — a real cross-package
 * boundary violation under default options (mirrors
 * `crates/cli/tests/cli.rs`'s `write_violation_fixture`).
 */
function writeFixture(dir) {
  const write = (relative, contents) => {
    const filePath = path.join(dir, relative);
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, contents);
  };
  write(
    "src/consumer.ts",
    'import { helper } from "./internal/util";\nconsole.log(helper);\n',
  );
  write("src/internal/util.ts", "/** @package */\nexport const helper = 1;\n");
}

main();

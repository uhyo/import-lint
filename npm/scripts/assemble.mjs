#!/usr/bin/env node
"use strict";

// Zero-dependency, Node 18+ compatible. Stamps the real release version into
// the seven checked-in package.jsons under npm/ and copies each target's
// binary into its platform package (docs/PLAN-npm.md §2, P6).
//
// Usage:
//   node assemble.mjs --version <x.y.z> --dist <dir> [--only <platform-key>] [--npm-root <dir>]
//
// --dist layout is flexible: for each platform key this looks for the binary
// at either `<dist>/<key>/import-lint(.exe)` or `<dist>/import-lint-<key>(.exe)`.
//
// --only assembles a single platform package (used by smoke.mjs for a local,
// host-only dry run) — every platform package.json still gets the version
// stamp, but only the binaries actually found (which is just the `--only` key
// in that mode) are copied.
//
// --npm-root overrides which npm/ tree to operate on (defaults to the real
// checked-in tree next to this script). smoke.mjs points this at a temp copy
// so local smoke runs never touch the working tree.
//
// Fails loudly and does nothing (no partial stamp, no partial copy) if any
// binary required for this run is missing — a half-release must be
// impossible (docs/PLAN-npm.md P5).

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export const PLATFORM_KEYS = [
  "darwin-arm64",
  "darwin-x64",
  "linux-arm64-gnu",
  "linux-x64-gnu",
  "linux-x64-musl",
  "win32-x64",
];

export function binaryFileName(platformKey) {
  return platformKey.startsWith("win32-") ? "import-lint.exe" : "import-lint";
}

export function parseArgs(argv) {
  const args = { version: undefined, dist: undefined, only: undefined, npmRoot: undefined };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    switch (arg) {
      case "--version":
        args.version = argv[++i];
        break;
      case "--dist":
        args.dist = argv[++i];
        break;
      case "--only":
        args.only = argv[++i];
        break;
      case "--npm-root":
        args.npmRoot = argv[++i];
        break;
      default:
        throw new Error(`assemble.mjs: unknown argument "${arg}"`);
    }
  }
  if (!args.version) {
    throw new Error("assemble.mjs: --version <x.y.z> is required");
  }
  if (!args.dist) {
    throw new Error("assemble.mjs: --dist <dir> is required");
  }
  if (args.only && !PLATFORM_KEYS.includes(args.only)) {
    throw new Error(
      `assemble.mjs: --only "${args.only}" is not one of: ${PLATFORM_KEYS.join(", ")}`,
    );
  }
  return args;
}

/** Returns the binary path for `key` under `distDir`, or `null` if absent. */
export function findBinary(distDir, key) {
  const fileName = binaryFileName(key);
  const flatSuffix = key.startsWith("win32-") ? ".exe" : "";
  const candidates = [
    path.join(distDir, key, fileName),
    path.join(distDir, `import-lint-${key}${flatSuffix}`),
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function writeJson(file, data) {
  fs.writeFileSync(file, `${JSON.stringify(data, null, 2)}\n`);
}

export function assemble({ version, dist, only, npmRoot }) {
  const root = npmRoot ?? path.resolve(__dirname, "..");
  const keysToCopy = only ? [only] : PLATFORM_KEYS;

  const missing = [];
  const foundBinaries = new Map();
  for (const key of keysToCopy) {
    const binPath = findBinary(dist, key);
    if (binPath) {
      foundBinaries.set(key, binPath);
    } else {
      missing.push(key);
    }
  }
  if (missing.length > 0) {
    throw new Error(
      `assemble.mjs: missing binaries for: ${missing.join(", ")}\n` +
        `Looked in "${dist}" for <key>/${"import-lint(.exe)"} or import-lint-<key>(.exe).`,
    );
  }

  const mainPkgPath = path.join(root, "import-lint", "package.json");
  const mainPkg = readJson(mainPkgPath);
  mainPkg.version = version;
  for (const key of PLATFORM_KEYS) {
    const depName = `@import-lint/${key}`;
    if (mainPkg.optionalDependencies && depName in mainPkg.optionalDependencies) {
      mainPkg.optionalDependencies[depName] = version;
    }
  }
  writeJson(mainPkgPath, mainPkg);

  for (const key of PLATFORM_KEYS) {
    const pkgPath = path.join(root, "platform", key, "package.json");
    const pkg = readJson(pkgPath);
    pkg.version = version;
    writeJson(pkgPath, pkg);
  }

  for (const [key, binPath] of foundBinaries) {
    const destPath = path.join(root, "platform", key, binaryFileName(key));
    fs.copyFileSync(binPath, destPath);
    fs.chmodSync(destPath, 0o755);
  }

  return { root, assembled: [...foundBinaries.keys()] };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const dist = path.resolve(args.dist);
  const npmRoot = args.npmRoot ? path.resolve(args.npmRoot) : undefined;
  const { root, assembled } = assemble({
    version: args.version,
    dist,
    only: args.only,
    npmRoot,
  });
  console.log(
    `assemble.mjs: stamped version ${args.version} in "${root}"; assembled binaries for: ${assembled.join(", ")}`,
  );
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url));
if (isMain) {
  try {
    main();
  } catch (err) {
    console.error(err.message ?? err);
    process.exit(1);
  }
}

#!/usr/bin/env node
// Conformance oracle for ImportLint.
//
// Runs the REFERENCE implementation (eslint-plugin-import-access) over its own
// fixture project, for every option set exercised by its own test suite, and
// dumps the resulting diagnostics as JSON snapshots under ../expected/.
//
// These snapshots are the ground truth that the Rust implementation is diffed
// against (see docs/PLAN-v1.md §9.1). See ../README.md for usage.
//
// Usage:
//   node generate-snapshots.mjs [path-to-reference-repo-checkout]
//   REFERENCE_REPO=/path/to/eslint-plugin-import-access node generate-snapshots.mjs
//
// Env:
//   SKIP_BUILD=1   skip `npm run build` in the reference repo (use existing dist/)

import { createRequire } from "node:module";
import { execFileSync, execSync } from "node:child_process";
import path from "node:path";
import fs from "node:fs";
import { fileURLToPath } from "node:url";

const EXPECTED_PLUGIN_VERSION = "3.1.0";

const referenceRepoPath = path.resolve(
  process.argv[2] ??
    process.env.REFERENCE_REPO ??
    "/home/uhyo/repos/eslint-plugin-import-access",
);

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.resolve(scriptDir, "../expected");

function log(...args) {
  console.log("[oracle]", ...args);
}

// --- sanity checks -------------------------------------------------------

const pkgJsonPath = path.join(referenceRepoPath, "package.json");
if (!fs.existsSync(pkgJsonPath)) {
  throw new Error(
    `Reference repo not found at ${referenceRepoPath} (no package.json). ` +
      `Pass its path as the first CLI arg or set REFERENCE_REPO.`,
  );
}
const pkgJson = JSON.parse(fs.readFileSync(pkgJsonPath, "utf8"));
if (pkgJson.name !== "eslint-plugin-import-access") {
  throw new Error(
    `${referenceRepoPath} does not look like eslint-plugin-import-access (package.json name is "${pkgJson.name}")`,
  );
}
if (pkgJson.version !== EXPECTED_PLUGIN_VERSION) {
  throw new Error(
    `Expected reference plugin version ${EXPECTED_PLUGIN_VERSION}, found ${pkgJson.version}. ` +
      `Update EXPECTED_PLUGIN_VERSION (and re-verify the whole conformance suite) if this is intentional.`,
  );
}

const refGitStatusBefore = execFileSync(
  "git",
  ["-C", referenceRepoPath, "status", "--porcelain"],
  { encoding: "utf8" },
);

if (!process.env.SKIP_BUILD) {
  log("Building reference plugin (npm run build)...");
  execSync("npm run build", { cwd: referenceRepoPath, stdio: "inherit" });
} else {
  log("SKIP_BUILD set, using existing dist/ as-is.");
}

// --- load the reference plugin's own dependencies -------------------------
// We must resolve @typescript-eslint/parser, @typescript-eslint/utils, and
// the built rule from the REFERENCE repo's node_modules/dist, not from
// wherever this script happens to live. createRequire rooted inside the
// reference repo gives us exactly that resolution behavior.
const req = createRequire(path.join(referenceRepoPath, "noop.cjs"));

const { TSESLint } = req("@typescript-eslint/utils");
const parser = req("@typescript-eslint/parser");
const jsdocRule = req(path.join(referenceRepoPath, "dist/rules/jsdoc.js"))
  .default;

const fixtureRoot = path.join(
  referenceRepoPath,
  "src/__tests__/fixtures/project",
);

// --- enumerate fixture files via `git ls-files` ---------------------------
// Deliberately NOT a filesystem walk: other agents may be creating temporary
// untracked files in the reference repo checkout concurrently, and those
// must never leak into the oracle's fixture-file list.
const lsOutput = execFileSync(
  "git",
  ["-C", referenceRepoPath, "ls-files", "src/__tests__/fixtures/project/src"],
  { encoding: "utf8" },
);
const files = lsOutput
  .split("\n")
  .filter((f) => f.endsWith(".ts") || f.endsWith(".tsx"))
  .map((f) => path.join(referenceRepoPath, f))
  .sort();

if (files.length === 0) {
  throw new Error("No fixture .ts files found — enumeration is broken.");
}
log(`Enumerated ${files.length} fixture source files under ${fixtureRoot}`);

// --- linter setup (mirrors src/__tests__/fixtures/eslint.ts FlatESLintTester) --

const flatPlugin = { rules: { jsdoc: jsdocRule } };
const linter = new TSESLint.Linter({ cwd: fixtureRoot, configType: "flat" });

function lintFile(absPath, jsdocOptions) {
  const code = fs.readFileSync(absPath, "utf8");
  return linter.verify(
    code,
    {
      files: ["**/*.ts"],
      languageOptions: {
        parser,
        parserOptions: {
          // Repeated verify() calls against one Linter instance need this
          // disabled, per the reference test helper's own comment.
          disallowAutomaticSingleRunInference: true,
          ecmaVersion: 2020,
          tsconfigRootDir: fixtureRoot,
          projectService: true,
          sourceType: "module",
        },
      },
      plugins: { "import-access": flatPlugin },
      rules: { "import-access/jsdoc": ["error", jsdocOptions] },
    },
    { filename: absPath },
  );
}

// --- option sets ------------------------------------------------------
// Every distinct options object passed to `tester.lintFile(...)` anywhere in
// src/__tests__/*.ts (excluding fixtures/), canonicalized. See
// tests/conformance/README.md for how this list was derived.

const optionSets = [
  { name: "default", options: {} },
  { name: "index-loophole-false", options: { indexLoophole: false } },
  {
    name: "index-loophole-false-filename-loophole-true",
    options: { indexLoophole: false, filenameLoophole: true },
  },
  {
    name: "default-importability-package",
    options: { defaultImportability: "package" },
  },
  {
    name: "default-importability-package-exclude-source-patterns",
    options: {
      defaultImportability: "package",
      excludeSourcePatterns: ["src/exclude-patterns/types/**"],
    },
  },
  {
    name: "default-importability-private",
    options: { defaultImportability: "private" },
  },
  {
    name: "default-importability-private-self-reference-internal",
    options: {
      defaultImportability: "private",
      treatSelfReferenceAs: "internal",
    },
  },
  {
    name: "default-importability-private-self-reference-external",
    options: {
      defaultImportability: "private",
      treatSelfReferenceAs: "external",
    },
  },
  {
    name: "package-directory-no-internal",
    options: { packageDirectory: ["**", "!**/_internal"] },
  },
  {
    name: "package-directory-all-star",
    options: { packageDirectory: ["**"] },
  },
  {
    name: "package-directory-no-internal-filename-loophole",
    options: {
      packageDirectory: ["**", "!**/_internal"],
      filenameLoophole: true,
    },
  },
  {
    // CAUTION: the checked-in expected/package-directory-packages-glob.json is
    // NOT this script's verbatim output — ImportLint deliberately diverges from
    // the reference for files outside every packageDirectory match (project-root
    // fallback instead of parent-directory fallback). After regenerating,
    // re-apply the divergence documented in ../README.md to that one file.
    name: "package-directory-packages-glob",
    options: { packageDirectory: ["src/package-directory/packages/*"] },
  },
];

// --- run ------------------------------------------------------------------

const IDENT_RE = /'([^']*)'\s*$/;

fs.mkdirSync(outDir, { recursive: true });

const manifest = {
  referenceRepo: {
    name: pkgJson.name,
    version: pkgJson.version,
  },
  generatedAt: new Date().toISOString(),
  fixtureRoot: "tests/conformance/fixtures/project",
  optionSets: {},
};

for (const { name, options } of optionSets) {
  const diagnostics = [];
  for (const absPath of files) {
    const relFile = path
      .relative(fixtureRoot, absPath)
      .split(path.sep)
      .join("/");
    const messages = lintFile(absPath, options);
    for (const m of messages) {
      if (m.messageId === "no-program") {
        throw new Error(
          `Unexpected "no-program" diagnostic in ${relFile} under option set "${name}" — ` +
            `typed linting isn't set up correctly for this file.`,
        );
      }
      const identMatch = IDENT_RE.exec(m.message);
      diagnostics.push({
        file: relFile,
        line: m.line,
        column: m.column,
        endLine: m.endLine,
        endColumn: m.endColumn,
        messageId: m.messageId,
        message: m.message,
        identifier: identMatch ? identMatch[1] : null,
      });
    }
  }

  diagnostics.sort((a, b) => {
    if (a.file !== b.file) return a.file < b.file ? -1 : 1;
    if (a.line !== b.line) return a.line - b.line;
    if (a.column !== b.column) return a.column - b.column;
    if (a.messageId !== b.messageId)
      return a.messageId < b.messageId ? -1 : 1;
    return 0;
  });

  const outFileName = `${name}.json`;
  fs.writeFileSync(
    path.join(outDir, outFileName),
    JSON.stringify(diagnostics, null, 2) + "\n",
  );
  manifest.optionSets[name] = {
    file: outFileName,
    options,
    diagnosticCount: diagnostics.length,
  };
  log(`${name}: ${diagnostics.length} diagnostics`);
}

fs.writeFileSync(
  path.join(outDir, "manifest.json"),
  JSON.stringify(manifest, null, 2) + "\n",
);

// --- verify we left no mess in the reference repo --------------------------
const refGitStatusAfter = execFileSync(
  "git",
  ["-C", referenceRepoPath, "status", "--porcelain"],
  { encoding: "utf8" },
);
if (refGitStatusAfter !== refGitStatusBefore) {
  log(
    "WARNING: reference repo git status changed during the run (likely just dist/ which is gitignored). Diff:",
  );
  log(refGitStatusAfter);
}

log("Done.");

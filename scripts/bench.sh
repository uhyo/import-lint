#!/usr/bin/env bash
# End-to-end benchmark for import-lint (PLAN-v1.md §8, M7).
#
# Usage:
#   scripts/bench.sh [--compare-eslint] [--seed N] [--files-5k N] [--files-10k N]
#
# Builds --release, generates (or reuses) synthetic 5k/10k-file trees via
# gen-fixture, times `import-lint <tree>` on each with hyperfine (falling back to
# a plain 5-run `time` loop if hyperfine isn't installed), and prints a markdown
# summary table.
#
# --compare-eslint additionally times the reference eslint-plugin-import-access
# (checked out read-only at ~/repos/eslint-plugin-import-access) over the 5k tree,
# in a throwaway npm project created OUTSIDE that checkout. This step is slow
# (ESLint + full type-aware parsing) and degrades gracefully to printing manual
# instructions if npm/network isn't cooperative — it never fails the rest of the
# script.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

COMPARE_ESLINT=0
SEED=42
FILES_5K=5000
FILES_10K=10000

while [ $# -gt 0 ]; do
  case "$1" in
    --compare-eslint) COMPARE_ESLINT=1 ;;
    --seed) SEED="$2"; shift ;;
    --files-5k) FILES_5K="$2"; shift ;;
    --files-10k) FILES_10K="$2"; shift ;;
    *) echo "bench.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

REFERENCE_CHECKOUT="${IMPORT_LINT_BENCH_REFERENCE:-$HOME/repos/eslint-plugin-import-access}"
CACHE_DIR="${IMPORT_LINT_BENCH_CACHE:-/tmp/import-lint-bench-cache}"
mkdir -p "$CACHE_DIR"

log() { echo "==> $*" >&2; }

log "Building --release (import-lint, gen-fixture)..."
cargo build --release --bin import-lint --bin gen-fixture

IMPORT_LINT_BIN="$ROOT_DIR/target/release/import-lint"
GEN_FIXTURE_BIN="$ROOT_DIR/target/release/gen-fixture"

# ---- fixture generation (cached by files/seed) ----

# Prints the tree's directory path on stdout; all status goes to stderr so
# `dir="$(gen_tree N)"` captures only the path.
gen_tree() {
  local files="$1"
  local dir="$CACHE_DIR/tree-${files}-seed${SEED}"
  local marker="$dir/.gen-fixture-done"
  if [ -f "$marker" ]; then
    log "Reusing cached ${files}-file tree at $dir"
  else
    rm -rf "$dir"
    log "Generating ${files}-file synthetic tree at $dir..."
    "$GEN_FIXTURE_BIN" "$dir" --files "$files" --seed "$SEED" >&2
    touch "$marker"
  fi
  echo "$dir"
}

TREE_5K="$(gen_tree "$FILES_5K")"
TREE_10K="$(gen_tree "$FILES_10K")"

count_ts_files() {
  find "$1" \( -name '*.ts' -o -name '*.tsx' \) -type f | wc -l | tr -d ' '
}

# ---- timing: hyperfine if available, else a 5-run `time` loop ----

HAVE_HYPERFINE=0
if command -v hyperfine >/dev/null 2>&1; then
  HAVE_HYPERFINE=1
  log "Using hyperfine ($(hyperfine --version))"
else
  log "hyperfine not found on PATH; falling back to a 5-run time loop"
fi

# Appends one row to $SUMMARY_ROWS (a newline-separated list of markdown table
# rows) timing `$IMPORT_LINT_BIN <dir>`.
bench_import_lint() {
  local label="$1"
  local dir="$2"
  log "Benchmarking: $label ($dir, $(count_ts_files "$dir") .ts files)"

  if [ "$HAVE_HYPERFINE" -eq 1 ]; then
    local out
    out="$(hyperfine --warmup 1 --ignore-failure "$IMPORT_LINT_BIN '$dir'" 2>&1 | tee /dev/stderr)"
    local mean_line range_line
    mean_line="$(echo "$out" | grep -oP 'Time \(mean.*' | head -1 | sed -E 's/^Time \(mean ± σ\):\s*//; s/\s*\[.*$//')"
    range_line="$(echo "$out" | grep -oP 'Range \(min … max\).*' | head -1 | sed 's/^Range (min … max):\s*//')"
    SUMMARY_ROWS+="| $label | $(count_ts_files "$dir") | import-lint (hyperfine) | ${mean_line:-n/a} | ${range_line:-n/a} |"$'\n'
  else
    local times=()
    local i
    for i in 1 2 3 4 5; do
      local start end
      start="$(date +%s%N)"
      "$IMPORT_LINT_BIN" "$dir" >/dev/null 2>&1 || true
      end="$(date +%s%N)"
      times+=("$(( (end - start) / 1000000 ))")
    done
    log "  runs (ms): ${times[*]}"
    local sorted
    sorted="$(printf '%s\n' "${times[@]}" | sort -n)"
    local min median max
    min="$(echo "$sorted" | sed -n '1p')"
    median="$(echo "$sorted" | sed -n '3p')"
    max="$(echo "$sorted" | sed -n '5p')"
    SUMMARY_ROWS+="| $label | $(count_ts_files "$dir") | import-lint (time, 5 runs) | median ${median} ms | min ${min} ms / max ${max} ms |"$'\n'
  fi
}

SUMMARY_ROWS=""
SUMMARY_ROWS+="| Tree | .ts files | Tool | Mean / median | Range |"$'\n'
SUMMARY_ROWS+="|---|---|---|---|---|"$'\n'

bench_import_lint "5k" "$TREE_5K"
bench_import_lint "10k" "$TREE_10K"

# ---- optional: reference ESLint plugin comparison (5k tree only) ----

compare_eslint() {
  local dir="$1"

  if [ ! -d "$REFERENCE_CHECKOUT" ]; then
    log "ESLint comparison skipped: $REFERENCE_CHECKOUT does not exist."
    print_manual_eslint_instructions "$dir"
    return 0
  fi
  if [ ! -f "$REFERENCE_CHECKOUT/dist/index.js" ]; then
    log "ESLint comparison: $REFERENCE_CHECKOUT/dist is missing a build; attempting 'npm run build' there (its output, dist/, is gitignored)..."
    if ! (cd "$REFERENCE_CHECKOUT" && npm run build); then
      log "ESLint comparison skipped: build failed."
      print_manual_eslint_instructions "$dir"
      return 0
    fi
  fi

  local eslint_ver parser_ver ts_ver
  eslint_ver="$(node -e "console.log(require('$REFERENCE_CHECKOUT/node_modules/eslint/package.json').version)" 2>/dev/null || echo '')"
  parser_ver="$(node -e "console.log(require('$REFERENCE_CHECKOUT/node_modules/@typescript-eslint/parser/package.json').version)" 2>/dev/null || echo '')"
  ts_ver="$(node -e "console.log(require('$REFERENCE_CHECKOUT/node_modules/typescript/package.json').version)" 2>/dev/null || echo '')"
  if [ -z "$eslint_ver" ] || [ -z "$parser_ver" ] || [ -z "$ts_ver" ]; then
    log "ESLint comparison skipped: couldn't determine eslint/@typescript-eslint/parser/typescript versions from $REFERENCE_CHECKOUT/node_modules (run 'npm install' there first)."
    print_manual_eslint_instructions "$dir"
    return 0
  fi
  log "Reference checkout versions: eslint@$eslint_ver, @typescript-eslint/parser@$parser_ver, typescript@$ts_ver"

  local tmp_project
  tmp_project="$(mktemp -d -t import-lint-bench-eslint-XXXXXX)"
  log "Setting up a throwaway npm project at $tmp_project (outside the reference checkout)..."

  cat > "$tmp_project/package.json" <<EOF
{
  "name": "import-lint-bench-eslint-compare",
  "private": true,
  "type": "module"
}
EOF

  if ! npm install --prefix "$tmp_project" --no-audit --no-fund --save-exact \
      "eslint@$eslint_ver" \
      "@typescript-eslint/parser@$parser_ver" \
      "typescript@$ts_ver" \
      "eslint-plugin-import-access@file:$REFERENCE_CHECKOUT" \
      >&2; then
    log "ESLint comparison skipped: npm install failed (offline? registry unreachable?)."
    print_manual_eslint_instructions "$dir"
    rm -rf "$tmp_project"
    return 0
  fi

  cat > "$tmp_project/eslint.config.mjs" <<EOF
import typescriptEslintParser from "@typescript-eslint/parser";
import importAccess from "eslint-plugin-import-access/flat-config";

export default [
  {
    files: ["**/*.ts", "**/*.tsx"],
    languageOptions: {
      parser: typescriptEslintParser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: "$dir",
        sourceType: "module",
        ecmaVersion: "latest",
      },
    },
    plugins: {
      "import-access": importAccess,
    },
    rules: {
      "import-access/jsdoc": ["error"],
    },
  },
];
EOF

  local eslint_bin="$tmp_project/node_modules/.bin/eslint"
  if [ ! -x "$eslint_bin" ]; then
    log "ESLint comparison skipped: eslint binary not found after install."
    print_manual_eslint_instructions "$dir"
    rm -rf "$tmp_project"
    return 0
  fi

  log "Running the reference ESLint plugin over $dir (this is slow — type-aware parsing over thousands of files)..."
  # ESLint 9's flat config rejects lint targets outside the config file's "base
  # path" (it errors out instantly with "all files matching the glob pattern are
  # ignored" rather than actually linting anything) when `eslint.config.mjs` and
  # the linted directory live in different trees, as they deliberately do here
  # (config in the throwaway npm project, code in the cache dir). Running with
  # `cwd` set to the fixture directory and `.` as the target — while still
  # pointing `--config` at the absolute path of our config — sidesteps that.
  local eslint_row
  if [ "$HAVE_HYPERFINE" -eq 1 ]; then
    local out
    if ! out="$(hyperfine --runs 3 --ignore-failure --warmup 0 \
        --command-name "eslint --config .../eslint.config.mjs ." \
        "cd '$dir' && '$eslint_bin' --config '$tmp_project/eslint.config.mjs' ." 2>&1 | tee /dev/stderr)"; then
      log "ESLint comparison failed to run under hyperfine."
      print_manual_eslint_instructions "$dir"
      rm -rf "$tmp_project"
      return 0
    fi
    local mean_line range_line
    mean_line="$(echo "$out" | grep -oP 'Time \(mean.*' | head -1 | sed -E 's/^Time \(mean ± σ\):\s*//; s/\s*\[.*$//')"
    range_line="$(echo "$out" | grep -oP 'Range \(min … max\).*' | head -1 | sed 's/^Range (min … max):\s*//')"
    eslint_row="| 5k | $(count_ts_files "$dir") | eslint-plugin-import-access (hyperfine, 3 runs) | ${mean_line:-n/a} | ${range_line:-n/a} |"
  else
    local start end elapsed_ms
    start="$(date +%s%N)"
    (cd "$dir" && "$eslint_bin" --config "$tmp_project/eslint.config.mjs" . >/dev/null 2>&1) || true
    end="$(date +%s%N)"
    elapsed_ms="$(( (end - start) / 1000000 ))"
    eslint_row="| 5k | $(count_ts_files "$dir") | eslint-plugin-import-access (single run) | ${elapsed_ms} ms | n/a |"
  fi
  SUMMARY_ROWS+="${eslint_row}"$'\n'

  rm -rf "$tmp_project"
}

print_manual_eslint_instructions() {
  local dir="$1"
  cat >&2 <<EOF

To compare against the reference ESLint plugin manually:
  1. cd $REFERENCE_CHECKOUT && npm install && npm run build   # if not already done
  2. Create a throwaway npm project OUTSIDE that checkout:
       mkdir /tmp/eslint-compare && cd /tmp/eslint-compare
       npm init -y
       npm install eslint @typescript-eslint/parser typescript \\
         eslint-plugin-import-access@file:$REFERENCE_CHECKOUT
  3. Write eslint.config.mjs there importing "eslint-plugin-import-access/flat-config",
     with parserOptions.projectService: true and tsconfigRootDir: "$dir".
  4. Time it: hyperfine --runs 3 "node_modules/.bin/eslint --config eslint.config.mjs $dir"
EOF
}

if [ "$COMPARE_ESLINT" -eq 1 ]; then
  compare_eslint "$TREE_5K"
else
  log "Skipping ESLint comparison (pass --compare-eslint to run it)."
fi

echo
echo "## import-lint benchmark results"
echo
echo "Machine: $(uname -s) $(uname -r), $(nproc) cores"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo
printf '%s' "$SUMMARY_ROWS"

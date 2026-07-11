//! Criterion micro-benchmark for [`import_lint::extract_file`] (PLAN.md §8, M7):
//! parse+extract cost dominates the pipeline's per-file work (link and check are
//! cheap by comparison, PLAN.md §8), so this is the benchmark to watch for
//! extraction regressions independent of I/O/discovery/resolution noise.
//!
//! `REPRESENTATIVE_SOURCE` is a hand-written ~150-line `.ts` file mixing every
//! export form the extractor handles (spec §3.2) with JSDoc access tags on roughly
//! a third of them, meant to stand in for a typical file in a real project rather
//! than a minimal/synthetic one.
//!
//! Run with `cargo bench -p import_lint --bench extract` (add `-- --save-baseline
//! <name>` to compare across changes).

use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use import_lint::extract_file;
use oxc_allocator::Allocator;
use oxc_span::SourceType;

const REPRESENTATIVE_SOURCE: &str = r#"
import { readFile } from "node:fs/promises";
import { EventEmitter } from "node:events";
import type { Logger } from "./logger";
import { formatDuration, parseDuration } from "./duration";
import defaultConfig, { ConfigSchema } from "./config";
import * as pathUtils from "./path-utils";

export const DEFAULT_TIMEOUT_MS = 30_000;

/** @package */
export const MAX_RETRIES = 5;

/** @private */
const internalCounter = { value: 0 };

/**
 * Increment and return the module-local counter. Not exported.
 */
function nextId(): number {
  internalCounter.value += 1;
  return internalCounter.value;
}

export function createTimer(label: string): () => number {
  const start = Date.now();
  return () => Date.now() - start;
}

/** @package */
export async function loadConfig(path: string): Promise<ConfigSchema> {
  const text = await readFile(path, "utf8");
  return JSON.parse(text) as ConfigSchema;
}

export function debounce<Args extends unknown[]>(
  fn: (...args: Args) => void,
  waitMs: number,
): (...args: Args) => void {
  let handle: ReturnType<typeof setTimeout> | undefined;
  return (...args: Args) => {
    if (handle) clearTimeout(handle);
    handle = setTimeout(() => fn(...args), waitMs);
  };
}

/** @private */
export class RetryBudget {
  private remaining: number;

  constructor(initial: number = MAX_RETRIES) {
    this.remaining = initial;
  }

  consume(): boolean {
    if (this.remaining <= 0) return false;
    this.remaining -= 1;
    return true;
  }

  static withDefaults(): RetryBudget {
    return new RetryBudget();
  }
}

export class TaskQueue extends EventEmitter {
  private items: Array<() => Promise<void>> = [];
  private running = false;

  enqueue(task: () => Promise<void>): void {
    this.items.push(task);
    void this.drain();
  }

  private async drain(): Promise<void> {
    if (this.running) return;
    this.running = true;
    while (this.items.length > 0) {
      const task = this.items.shift();
      if (task) await task();
    }
    this.running = false;
  }
}

/** @package */
export interface RequestOptions {
  timeoutMs?: number;
  retries?: number;
  headers?: Record<string, string>;
}

export interface ResponseEnvelope<T> {
  status: number;
  body: T;
  durationMs: number;
}

/** @private */
export type RetryPolicy = "none" | "linear" | "exponential";

export type AsyncResult<T> = Promise<{ ok: true; value: T } | { ok: false; error: Error }>;

export const enum LogLevel {
  Debug = 0,
  Info = 1,
  Warn = 2,
  Error = 3,
}

/** @package */
export namespace Metrics {
  export const eventCount = { value: 0 };
  export function record(name: string): void {
    eventCount.value += 1;
  }
}

/**
 * @private
 */
export default function createClient(options: RequestOptions = {}): TaskQueue {
  const queue = new TaskQueue();
  const budget = RetryBudget.withDefaults();
  const timer = createTimer("client");
  void nextId();
  void formatDuration;
  void parseDuration;
  void defaultConfig;
  void pathUtils.join;
  void budget.consume();
  void timer();
  return queue;
}

export { formatDuration as reExportedFormatDuration } from "./duration";
export { default as ConfigDefault } from "./config";

/** @package */
export { RetryPolicyLabels } from "./labels";

export * from "./errors";
export * as pathHelpers from "./path-utils";
"#;

fn bench_extract(c: &mut Criterion) {
    let path = Path::new("representative.ts");
    let source_type = SourceType::ts();

    c.bench_function("extract/representative_file", |b| {
        b.iter(|| {
            let allocator = Allocator::default();
            black_box(extract_file(
                black_box(path),
                black_box(REPRESENTATIVE_SOURCE),
                source_type,
                &allocator,
            ))
        });
    });
}

criterion_group!(benches, bench_extract);
criterion_main!(benches);

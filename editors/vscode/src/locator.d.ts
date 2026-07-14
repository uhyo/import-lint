// Type declarations for locator.js (kept in sync by hand — the module itself
// is plain CommonJS so it can be unit-tested with `node --test`).

export type LocateSuccess = {
  ok: true;
  path: string;
  source: "settings" | "workspace" | "path";
};

export type LocateFailureReason =
  | "settings-path-missing"
  | "platform-package-missing"
  | "not-found";

export type LocateFailure = {
  ok: false;
  reason: LocateFailureReason;
  detail?: string;
};

export type LocateResult = LocateSuccess | LocateFailure;

export interface LocateBinaryOptions {
  settingsBinaryPath?: string;
  workspaceRoot?: string;
  platformKey?: string;
  env?: NodeJS.ProcessEnv;
  existsSync?: (path: string) => boolean;
  createRequire?: (filename: string) => NodeJS.Require;
}

export function locateBinary(options: LocateBinaryOptions): LocateResult;

export function parseVersionOutput(stdout: string | null | undefined): string | null;

export function isVersionAtLeast(version: string, minimum: string): boolean;

export const MIN_LSP_VERSION: string;

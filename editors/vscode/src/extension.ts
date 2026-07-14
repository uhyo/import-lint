import * as vscode from "vscode";
import { execFile } from "node:child_process";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";
import { locateBinary, parseVersionOutput, isVersionAtLeast, MIN_LSP_VERSION } from "./locator";

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel | undefined;

const VERSION_CHECK_TIMEOUT_MS = 5000;

type EnabledSetting = "auto" | "on" | "off";
type RunSetting = "onType" | "onSave";

function getConfig() {
  return vscode.workspace.getConfiguration("importLint");
}

function log(message: string): void {
  outputChannel?.appendLine(message);
}

function getVersion(binPath: string): Promise<string | null> {
  return new Promise((resolve) => {
    try {
      execFile(
        binPath,
        ["--version"],
        { timeout: VERSION_CHECK_TIMEOUT_MS },
        (error, stdout) => {
          if (error) {
            resolve(null);
            return;
          }
          resolve(parseVersionOutput(stdout));
        },
      );
    } catch {
      resolve(null);
    }
  });
}

async function maybeStart(context: vscode.ExtensionContext): Promise<void> {
  const config = getConfig();
  const enabled = config.get<EnabledSetting>("enabled", "auto");

  if (enabled === "off") {
    log("importLint.enabled is \"off\"; not starting the server.");
    return;
  }

  if (enabled === "auto") {
    const configFiles = await vscode.workspace.findFiles(
      "**/.importlintrc.{json,jsonc}",
      "**/node_modules/**",
      1,
    );
    if (configFiles.length === 0) {
      log(
        "importLint.enabled is \"auto\" and no .importlintrc.json(c) was found; not starting the server.",
      );
      return;
    }
  }

  const workspaceFolders = vscode.workspace.workspaceFolders;
  const workspaceRoot = workspaceFolders?.[0]?.uri.fsPath;
  if (workspaceFolders && workspaceFolders.length > 1) {
    log(
      `Warning: multiple workspace folders detected; using the first (${workspaceRoot}).`,
    );
  }

  const settingsBinaryPathRaw = config.get<string>("binaryPath", "");
  const settingsBinaryPath =
    settingsBinaryPathRaw && settingsBinaryPathRaw.length > 0
      ? settingsBinaryPathRaw
      : undefined;

  const located = locateBinary({ settingsBinaryPath, workspaceRoot });

  if (!located.ok) {
    let message: string;
    switch (located.reason) {
      case "settings-path-missing":
        message = `importLint.binaryPath points to ${located.detail} but it does not exist.`;
        break;
      case "platform-package-missing":
      case "not-found":
      default:
        message =
          "import-lint binary not found. Install with: npm install -D @import-lint/cli — or set importLint.binaryPath.";
        break;
    }
    log(`Binary resolution failed (${located.reason}).`);
    void vscode.window.showWarningMessage(message);
    return;
  }

  log(`Resolved import-lint binary at ${located.path} (source: ${located.source}).`);

  const version = await getVersion(located.path);
  if (!version || !isVersionAtLeast(version, MIN_LSP_VERSION)) {
    const versionLabel = version ?? "unknown";
    void vscode.window.showWarningMessage(
      `import-lint ${versionLabel} does not support the LSP server (need >= ${MIN_LSP_VERSION}). Upgrade @import-lint/cli.`,
    );
    log(
      `Version gate failed: detected "${versionLabel}", need >= ${MIN_LSP_VERSION}. Not starting the server.`,
    );
    return;
  }

  log(`import-lint version ${version} passes the LSP version gate.`);

  const run = config.get<RunSetting>("run", "onType");

  const serverOptions: ServerOptions = {
    command: located.path,
    args: ["lsp"],
    transport: TransportKind.stdio,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "typescript" },
      { scheme: "file", language: "typescriptreact" },
      { scheme: "file", language: "javascript" },
      { scheme: "file", language: "javascriptreact" },
    ],
    initializationOptions: { run },
    outputChannel,
  };

  client = new LanguageClient(
    "importLint",
    "ImportLint",
    serverOptions,
    clientOptions,
  );

  context.subscriptions.push(client);
  await client.start();
  log("ImportLint language server started.");
}

async function stopClient(): Promise<void> {
  if (!client) {
    return;
  }
  const toStop = client;
  client = undefined;
  try {
    await toStop.stop();
  } catch (err) {
    log(`Error stopping ImportLint language server: ${String(err)}`);
  }
}

export function activate(context: vscode.ExtensionContext): void {
  outputChannel = vscode.window.createOutputChannel("ImportLint");
  context.subscriptions.push(outputChannel);

  context.subscriptions.push(
    vscode.commands.registerCommand("importLint.restart", async () => {
      log("Restarting ImportLint language server...");
      await stopClient();
      await maybeStart(context);
    }),
  );

  let configChangeNotified = false;
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (!event.affectsConfiguration("importLint") || configChangeNotified) {
        return;
      }
      configChangeNotified = true;
      void vscode.window
        .showInformationMessage(
          "ImportLint settings changed. Restart the server to apply them?",
          "Restart",
        )
        .then((choice) => {
          configChangeNotified = false;
          if (choice === "Restart") {
            void vscode.commands.executeCommand("importLint.restart");
          }
        });
    }),
  );

  void maybeStart(context);
}

export async function deactivate(): Promise<void> {
  await stopClient();
}

import { test as base, expect, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "child_process";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const VARIANCE_BIN = path.resolve(__dirname, "../../target/debug/variance");

interface BackendInfo {
  port: number;
  process: ChildProcess;
  dataDir: string;
}

/**
 * Start a real Variance backend in a temp directory.
 *
 * Generates an unencrypted identity, then starts the CLI with
 * `--listen 127.0.0.1:0` so the OS picks a free port. Parses the
 * port from the log output.
 */
async function startBackend(): Promise<BackendInfo> {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "variance-e2e-"));
  const identityPath = path.join(dataDir, "identity.json");

  // Generate identity (synchronous — fast)
  const gen = spawn(VARIANCE_BIN, ["identity", "generate", "-o", identityPath, "--force"], {
    env: { ...process.env, VARIANCE_DATA_DIR: dataDir, RUST_LOG: "variance=info" },
  });
  await new Promise<void>((resolve, reject) => {
    gen.on("close", (code) =>
      code === 0 ? resolve() : reject(new Error(`identity generate exited ${code}`))
    );
    gen.on("error", reject);
  });

  // Start the node
  const proc = spawn(VARIANCE_BIN, ["start", "--listen", "127.0.0.1:0"], {
    env: {
      ...process.env,
      VARIANCE_DATA_DIR: dataDir,
      RUST_LOG: "variance=info,tower_http=warn",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });

  // Parse port from stderr (tracing output goes to stderr)
  const port = await new Promise<number>((resolve, reject) => {
    let combined = "";
    const timeout = setTimeout(() => {
      proc.kill();
      reject(new Error(`Backend did not start within 15s. Output:\n${combined}`));
    }, 15_000);

    const onData = (chunk: Buffer) => {
      combined += chunk.toString();
      // Strip ANSI escape codes before matching
      const clean = combined.replace(/\x1b\[[0-9;]*m/g, "");
      // Match "HTTP API: http://127.0.0.1:XXXXX"
      const match = clean.match(/HTTP API:\s*http:\/\/127\.0\.0\.1:(\d+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(parseInt(match[1], 10));
      }
    };

    proc.stderr?.on("data", onData);
    proc.stdout?.on("data", onData);

    proc.on("error", (err) => {
      clearTimeout(timeout);
      reject(err);
    });
    proc.on("close", (code) => {
      clearTimeout(timeout);
      reject(new Error(`Backend exited early with code ${code}. Output:\n${combined}`));
    });
  });

  return { port, process: proc, dataDir };
}

function stopBackend(info: BackendInfo) {
  info.process.kill("SIGTERM");
  // Clean up temp dir
  try {
    fs.rmSync(info.dataDir, { recursive: true, force: true });
  } catch {
    // ignore cleanup failures
  }
}

// Read the mock script template once
const MOCK_SCRIPT_PATH = path.resolve(__dirname, "tauri-mock.js");

// Shared backend across all tests in a worker (started once, reused)
let _backend: BackendInfo | null = null;

async function getOrStartBackend(): Promise<BackendInfo> {
  if (!_backend) {
    _backend = await startBackend();
  }
  return _backend;
}

export type E2EFixtures = {
  /** The API port for the running backend */
  apiPort: number;
  /** Page with Tauri mocks and backend already configured */
  appPage: Page;
  /** Page with Tauri mocks but NOT pre-seeded as onboarded (for onboarding tests) */
  freshPage: Page;
};

/**
 * Build a Tauri mock init script with optional invoke overrides.
 *
 * Any command in `overrides` replaces the default handler.
 */
export function buildTauriMock(port: number, overrides: Record<string, string> = {}): string {
  const mockScript = fs.readFileSync(MOCK_SCRIPT_PATH, "utf-8");
  let injected = mockScript.replace(
    /var __E2E_API_PORT__ = __E2E_API_PORT__ \|\| 0;/,
    `var __E2E_API_PORT__ = ${port};`
  );
  // Inject overrides before the default switch
  if (Object.keys(overrides).length > 0) {
    const cases = Object.entries(overrides)
      .map(([cmd, body]) => `case ${JSON.stringify(cmd)}: ${body}`)
      .join("\n            ");
    injected = injected.replace("switch (cmd) {", `switch (cmd) {\n            ${cases}`);
  }
  return injected;
}

/**
 * Extended Playwright test with:
 * - `apiPort`: the HTTP API port of the running Variance backend
 * - `appPage`: a page with Tauri mocks injected and the app loaded
 *
 * A single backend is shared across all tests in the worker.
 */
export const test = base.extend<E2EFixtures>({
  apiPort: async ({}, use) => {
    const backend = await getOrStartBackend();
    await use(backend.port);
  },

  appPage: async ({ page, apiPort }, use) => {
    // Inject Tauri mock with the real port before app loads
    await page.addInitScript({ content: buildTauriMock(apiPort) });

    // Pre-seed localStorage so the app considers itself onboarded.
    // The identity store uses zustand persist with key "variance-identity"
    // and only partializes { identityPath, isOnboarded }.
    await page.addInitScript({
      content: `
        localStorage.setItem(
          'variance-identity',
          JSON.stringify({
            state: { identityPath: "/tmp/e2e-identity.json", isOnboarded: true },
            version: 0
          })
        );
      `,
    });

    await page.goto("/");
    await use(page);
  },

  freshPage: async ({ page, apiPort }, use) => {
    // Inject Tauri mock but do NOT seed localStorage — app starts in onboarding state.
    // Override generate_identity and recover_identity to return fake data.
    const overrides = {
      generate_identity:
        'return { did: "did:variance:e2e-generated", mnemonic: ["abandon","abandon","abandon","abandon","abandon","abandon","abandon","abandon","abandon","abandon","abandon","about"] };',
      recover_identity: 'return "did:variance:e2e-recovered";',
    };
    await page.addInitScript({ content: buildTauriMock(apiPort, overrides) });
    await page.goto("/");
    await use(page);
  },
});

// Clean up backend when the process exits
process.on("exit", () => {
  if (_backend) {
    _backend.process.kill("SIGTERM");
    try {
      fs.rmSync(_backend.dataDir, { recursive: true, force: true });
    } catch {
      // ignore
    }
  }
});

export { expect };

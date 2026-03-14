import { test, expect } from "./fixtures";

test.describe("App startup", () => {
  test("loads the main shell with conversation list", async ({ appPage }) => {
    // The app should transition from LoadingScreen to MainShell.
    // MainShell renders a ConversationList on the left and a placeholder on the right.
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });
  });

  test("health endpoint is reachable from the running backend", async ({ apiPort }) => {
    const res = await fetch(`http://127.0.0.1:${apiPort}/health`);
    expect(res.ok).toBe(true);
    const body = (await res.json()) as { status: string; service: string };
    expect(body.status).toBe("ok");
    expect(body.service).toBe("variance-app");
  });

  test("identity is fetched and displayed", async ({ appPage, apiPort }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Verify the backend returns a valid DID
    const res = await fetch(`http://127.0.0.1:${apiPort}/identity`);
    expect(res.ok).toBe(true);
    const identity = (await res.json()) as { did: string };
    expect(identity.did).toMatch(/^did:variance:/);
  });

  test("conversations endpoint returns valid data", async ({ apiPort }) => {
    const res = await fetch(`http://127.0.0.1:${apiPort}/conversations`);
    expect(res.ok).toBe(true);
    const conversations = (await res.json()) as { id: string; peer_did: string }[];
    expect(Array.isArray(conversations)).toBe(true);
    // Each conversation should have required fields
    for (const c of conversations) {
      expect(c.id).toBeTruthy();
      expect(c.peer_did).toBeTruthy();
    }
  });

  test("can create and list a group via API", async ({ apiPort }) => {
    // Count existing groups
    const beforeRes = await fetch(`http://127.0.0.1:${apiPort}/mls/groups`);
    const before = (await beforeRes.json()) as { id: string; name: string }[];
    const countBefore = before.length;

    // Create a new group
    const res = await fetch(`http://127.0.0.1:${apiPort}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "E2E Test Group" }),
    });
    expect(res.ok).toBe(true);
    const body = (await res.json()) as {
      success: boolean;
      group_id: string;
      name: string;
    };
    expect(body.success).toBe(true);
    expect(body.name).toBe("E2E Test Group");
    expect(body.group_id).toBeTruthy();

    // Verify the count increased
    const afterRes = await fetch(`http://127.0.0.1:${apiPort}/mls/groups`);
    const after = (await afterRes.json()) as { id: string; name: string }[];
    expect(after.length).toBe(countBefore + 1);
    expect(after.some((g) => g.id === body.group_id)).toBe(true);
  });

  test("presence endpoint returns valid data", async ({ apiPort }) => {
    const res = await fetch(`http://127.0.0.1:${apiPort}/presence`);
    expect(res.ok).toBe(true);
    const body = (await res.json()) as { online: string[] };
    expect(Array.isArray(body.online)).toBe(true);
  });

  test("invitations endpoint works", async ({ apiPort }) => {
    const res = await fetch(`http://127.0.0.1:${apiPort}/invitations`);
    expect(res.ok).toBe(true);
    const invitations = (await res.json()) as unknown[];
    expect(Array.isArray(invitations)).toBe(true);
  });

  test("config endpoints work", async ({ apiPort }) => {
    // Retention config
    const retRes = await fetch(`http://127.0.0.1:${apiPort}/config/retention`);
    expect(retRes.ok).toBe(true);
    const retention = (await retRes.json()) as { group_message_max_age_days: number };
    expect(typeof retention.group_message_max_age_days).toBe("number");

    // Relay config
    const relayRes = await fetch(`http://127.0.0.1:${apiPort}/config/relays`);
    expect(relayRes.ok).toBe(true);
    const relays = (await relayRes.json()) as unknown[];
    expect(Array.isArray(relays)).toBe(true);
  });
});

test.describe("Frontend rendering", () => {
  test("no console errors during startup", async ({ appPage }) => {
    const errors: string[] = [];
    appPage.on("console", (msg) => {
      if (msg.type() === "error") {
        // Ignore WebSocket connection failures and CORS errors
        const text = msg.text();
        if (text.includes("WebSocket")) return;
        if (text.includes("CORS")) return;
        if (text.includes("ERR_FAILED")) return;
        errors.push(text);
      }
    });

    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    await appPage.waitForTimeout(1_000);

    expect(errors).toEqual([]);
  });

  test("dark mode class is applied based on system preference", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    const htmlClass = await appPage.locator("html").getAttribute("class");
    // In default Playwright (light mode), dark should not be present
    expect(htmlClass ?? "").not.toContain("dark");
  });

  test("retry button appears on error state", async ({ page }) => {
    // Inject a mock where start_node throws an error
    const mockScript = `
      (function() {
        window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
        window.__TAURI_EVENT_PLUGIN_INTERNALS__ = window.__TAURI_EVENT_PLUGIN_INTERNALS__ || {};
        window.__TAURI_INTERNALS__.metadata = {
          currentWindow: { label: "main" },
          currentWebview: { windowLabel: "main", label: "main" },
        };
        var callbacks = new Map();
        var nextId = 1;
        window.__TAURI_INTERNALS__.transformCallback = function(cb, once) {
          var id = nextId++;
          callbacks.set(id, function(data) { if (once) callbacks.delete(id); return cb && cb(data); });
          return id;
        };
        window.__TAURI_INTERNALS__.unregisterCallback = function(id) { callbacks.delete(id); };
        window.__TAURI_INTERNALS__.runCallback = function(id, data) { var cb = callbacks.get(id); if (cb) cb(data); };
        window.__TAURI_INTERNALS__.callbacks = callbacks;
        window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener = function() {};

        window.__TAURI_INTERNALS__.invoke = async function(cmd) {
          switch (cmd) {
            case "default_identity_path": return "/tmp/e2e-identity.json";
            case "has_identity": return true;
            case "check_identity_encrypted": return false;
            case "start_node": throw "Simulated startup failure";
            case "get_api_port": return null;
            case "get_node_status": return { running: false, local_did: null, api_port: null };
            case "plugin:event|listen": return window.__TAURI_INTERNALS__.transformCallback(function(){});
            default: return null;
          }
        };
      })();
    `;
    await page.addInitScript({ content: mockScript });

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

    await expect(page.getByText("Failed to start node")).toBeVisible({
      timeout: 10_000,
    });

    await expect(page.getByText("Retry")).toBeVisible();
  });
});

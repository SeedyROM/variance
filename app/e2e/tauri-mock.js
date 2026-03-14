/**
 * Tauri mock init script — injected into the browser via page.addInitScript()
 * before the app loads.
 *
 * Mocks `window.__TAURI_INTERNALS__` and `window.__TAURI_EVENT_PLUGIN_INTERNALS__`
 * so the React app can run outside a real Tauri webview.
 *
 * The `__E2E_API_PORT__` placeholder is replaced at runtime by the fixture
 * with the actual backend port.
 */

// eslint-disable-next-line no-var
var __E2E_API_PORT__ = __E2E_API_PORT__ || 0;

(function () {
  // Build the mock internals objects
  window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = window.__TAURI_EVENT_PLUGIN_INTERNALS__ || {};

  // Metadata: fake "main" window so getCurrentWebviewWindow() / getCurrentWindow() work
  window.__TAURI_INTERNALS__.metadata = {
    currentWindow: { label: "main" },
    currentWebview: { windowLabel: "main", label: "main" },
  };

  // Callback registry (needed for Tauri's Channel mechanism)
  const callbacks = new Map();
  let nextId = 1;

  window.__TAURI_INTERNALS__.transformCallback = function (callback, once) {
    const id = nextId++;
    callbacks.set(id, function (data) {
      if (once) callbacks.delete(id);
      return callback && callback(data);
    });
    return id;
  };

  window.__TAURI_INTERNALS__.unregisterCallback = function (id) {
    callbacks.delete(id);
  };

  window.__TAURI_INTERNALS__.runCallback = function (id, data) {
    const cb = callbacks.get(id);
    if (cb) cb(data);
  };

  window.__TAURI_INTERNALS__.callbacks = callbacks;

  // Event plugin: no-op listener unregister
  window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener = function () {};

  // The main IPC mock — handles invoke() calls
  window.__TAURI_INTERNALS__.invoke = async function (cmd, args) {
    switch (cmd) {
      // Identity / startup flow
      case "default_identity_path":
        return "/tmp/e2e-identity.json";

      case "has_identity":
        return true;

      case "check_identity_encrypted":
        return false;

      case "start_node":
        return __E2E_API_PORT__;

      case "get_api_port":
        return __E2E_API_PORT__;

      case "get_node_status":
        return {
          running: true,
          local_did: "did:variance:e2e-test",
          api_port: __E2E_API_PORT__,
        };

      case "stop_node":
        return null;

      // Notification plugin commands — no-op in e2e
      case "plugin:notification|is_permission_granted":
        return true;

      case "plugin:notification|request_permission":
        return "granted";

      case "plugin:notification|notify":
      case "plugin:notification|register_action_types":
      case "plugin:notification|remove_active":
        return null;

      // Event plugin — listen/emit stubs
      case "plugin:event|listen":
        // Return a handler id so unlisten can work
        return window.__TAURI_INTERNALS__.transformCallback(function () {});

      case "plugin:event|emit":
      case "plugin:event|unlisten":
        return null;

      // Window / webview plugin commands
      case "plugin:window|start_dragging":
      case "plugin:webview_window|start_dragging":
        return null;

      // Focus / visibility
      case "plugin:window|is_focused":
        return true;

      default:
        console.warn("[E2E Tauri Mock] Unhandled invoke:", cmd, args);
        return null;
    }
  };
})();

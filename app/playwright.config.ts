import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  retries: 0,
  workers: 1, // serial: shared backend across tests
  use: {
    baseURL: "http://localhost:1420",
    trace: "on-first-retry",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        // Disable CORS: in production the Tauri webview doesn't enforce it,
        // but Playwright's Chromium does. The frontend at localhost:1420
        // fetches from 127.0.0.1:<backend-port>, which is cross-origin.
        launchOptions: {
          args: ["--disable-web-security"],
        },
      },
    },
  ],
  webServer: {
    command: "pnpm exec vite --port 1420",
    port: 1420,
    reuseExistingServer: !process.env.CI,
  },
});

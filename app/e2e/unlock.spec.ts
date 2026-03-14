import { test as base, expect } from "@playwright/test";
import { buildTauriMock } from "./fixtures";

// These tests use custom Tauri mocks to simulate the encrypted-identity flow.
// They don't need a real backend — the unlock screen is purely frontend logic
// interacting with Tauri invoke commands.

base.describe("Unlock screen", () => {
  base("shows unlock screen for encrypted identity", async ({ page }) => {
    // Mock: check_identity_encrypted returns true → needs-unlock state
    const mockScript = buildTauriMock(0, {
      check_identity_encrypted: "return true;",
    });
    await page.addInitScript({ content: mockScript });

    // Seed as onboarded
    await page.addInitScript({
      content: `
        localStorage.setItem(
          'variance-identity',
          JSON.stringify({
            state: { identityPath: "/tmp/e2e-encrypted.json", isOnboarded: true },
            version: 0
          })
        );
      `,
    });

    await page.goto("/");

    // Should show the unlock screen
    await expect(page.getByText("Welcome back")).toBeVisible({
      timeout: 10_000,
    });
    await expect(page.getByText("Enter your passphrase to unlock Variance")).toBeVisible();
    await expect(page.getByPlaceholder("Enter your passphrase")).toBeVisible();
    await expect(page.getByRole("button", { name: "Unlock" })).toBeVisible();
  });

  base("unlock button is disabled when passphrase is empty", async ({ page }) => {
    const mockScript = buildTauriMock(0, {
      check_identity_encrypted: "return true;",
    });
    await page.addInitScript({ content: mockScript });
    await page.addInitScript({
      content: `
        localStorage.setItem(
          'variance-identity',
          JSON.stringify({
            state: { identityPath: "/tmp/e2e-encrypted.json", isOnboarded: true },
            version: 0
          })
        );
      `,
    });

    await page.goto("/");
    await expect(page.getByText("Welcome back")).toBeVisible({
      timeout: 10_000,
    });

    // Unlock button should be disabled with empty input
    await expect(page.getByRole("button", { name: "Unlock" })).toBeDisabled();
  });

  base("wrong passphrase shows error message", async ({ page }) => {
    // start_node rejects with "Decryption failed" when the passphrase is wrong
    const mockScript = buildTauriMock(0, {
      check_identity_encrypted: "return true;",
      start_node: 'throw "Decryption failed: wrong passphrase";',
    });
    await page.addInitScript({ content: mockScript });
    await page.addInitScript({
      content: `
        localStorage.setItem(
          'variance-identity',
          JSON.stringify({
            state: { identityPath: "/tmp/e2e-encrypted.json", isOnboarded: true },
            version: 0
          })
        );
      `,
    });

    await page.goto("/");
    await expect(page.getByText("Welcome back")).toBeVisible({
      timeout: 10_000,
    });

    // Type a wrong passphrase
    await page.getByPlaceholder("Enter your passphrase").fill("wrongpass");
    await page.getByRole("button", { name: "Unlock" }).click();

    // Should show error
    await expect(page.getByText("Wrong passphrase", { exact: false })).toBeVisible({
      timeout: 5_000,
    });
  });

  base("successful unlock transitions to main shell", async ({ page }) => {
    // start_node succeeds and returns a port
    const mockScript = buildTauriMock(9999, {
      check_identity_encrypted: "return true;",
    });
    await page.addInitScript({ content: mockScript });
    await page.addInitScript({
      content: `
        localStorage.setItem(
          'variance-identity',
          JSON.stringify({
            state: { identityPath: "/tmp/e2e-encrypted.json", isOnboarded: true },
            version: 0
          })
        );
      `,
    });

    await page.goto("/");
    await expect(page.getByText("Welcome back")).toBeVisible({
      timeout: 10_000,
    });

    // Type the correct passphrase and unlock
    await page.getByPlaceholder("Enter your passphrase").fill("correctpass");
    await page.getByRole("button", { name: "Unlock" }).click();

    // The app should transition past the unlock screen.
    // It may show a loading state or an error (since port 9999 isn't a real backend),
    // but the unlock screen should disappear.
    await expect(page.getByText("Welcome back")).not.toBeVisible({
      timeout: 10_000,
    });
  });

  base("'Use a different identity' link resets to onboarding", async ({ page }) => {
    const mockScript = buildTauriMock(0, {
      check_identity_encrypted: "return true;",
    });
    await page.addInitScript({ content: mockScript });
    await page.addInitScript({
      content: `
        localStorage.setItem(
          'variance-identity',
          JSON.stringify({
            state: { identityPath: "/tmp/e2e-encrypted.json", isOnboarded: true },
            version: 0
          })
        );
      `,
    });

    await page.goto("/");
    await expect(page.getByText("Welcome back")).toBeVisible({
      timeout: 10_000,
    });

    // Click "Use a different identity"
    await page.getByText("Use a different identity").click();

    // Should go back to the onboarding welcome screen
    await expect(page.getByText("Welcome to Variance")).toBeVisible({
      timeout: 10_000,
    });
  });
});

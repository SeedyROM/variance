import { test, expect } from "./fixtures";

test.describe("Onboarding flow", () => {
  test("shows welcome screen when not onboarded", async ({ freshPage }) => {
    await expect(freshPage.getByText("Welcome to Variance")).toBeVisible({
      timeout: 10_000,
    });
    await expect(freshPage.getByRole("button", { name: "Create new identity" })).toBeVisible();
    await expect(
      freshPage.getByRole("button", { name: "Recover existing identity" })
    ).toBeVisible();
  });

  test("full onboarding: generate identity without passphrase", async ({ freshPage, apiPort }) => {
    // Step 1: Welcome — click "Create new identity"
    await expect(freshPage.getByText("Welcome to Variance")).toBeVisible({
      timeout: 10_000,
    });
    await freshPage.getByRole("button", { name: "Create new identity" }).click();

    // Step 2: Passphrase — skip it
    await expect(freshPage.getByText("Protect your identity")).toBeVisible();
    await freshPage.getByRole("button", { name: "Continue without passphrase" }).click();

    // Step 3: Generate identity
    await expect(freshPage.getByText("Generate Identity")).toBeVisible();
    await freshPage.getByRole("button", { name: "Generate my identity" }).click();

    // Step 3b: Mnemonic display
    await expect(freshPage.getByText("Your Recovery Phrase")).toBeVisible({ timeout: 10_000 });
    await expect(
      freshPage.getByText("This is the only time you will see these words")
    ).toBeVisible();

    // Verify 12 words are shown (the mock returns "abandon" x11 + "about")
    // "abandon" appears 11 times in separate elements — use .first() to avoid strict mode violation
    await expect(freshPage.getByText("abandon").first()).toBeVisible();
    await expect(freshPage.getByText("about")).toBeVisible();

    // Check the confirmation checkbox (click the label — custom Checkbox hides
    // the native input behind sr-only, so Playwright's .check() can't reach it)
    await freshPage.getByText("I have written down").click();

    // Click Continue
    await freshPage.getByRole("button", { name: "Continue" }).click();

    // Step 4: Setup Complete
    await expect(freshPage.getByText("Identity Ready")).toBeVisible();
    await expect(freshPage.getByRole("button", { name: "Start Variance" })).toBeVisible();

    // Click "Start Variance" — this invokes start_node (mocked)
    await freshPage.getByRole("button", { name: "Start Variance" }).click();

    // Step 5: Username step
    await expect(freshPage.getByText("Choose a Username")).toBeVisible({
      timeout: 10_000,
    });

    // Type a username
    await freshPage.getByPlaceholder("satoshi").fill("e2etester");

    // The preview should show the username
    await expect(freshPage.getByText("e2etester#")).toBeVisible();

    // Click "Claim username" — this calls POST /identity/username on the real backend
    await freshPage.getByRole("button", { name: "Claim username" }).click();

    // Should transition to the main app shell
    await expect(freshPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Verify the username was actually registered on the backend
    const res = await fetch(`http://127.0.0.1:${apiPort}/identity`);
    const identity = (await res.json()) as {
      username: string;
      discriminator: number;
    };
    expect(identity.username).toBe("e2etester");
    expect(identity.discriminator).toBeGreaterThan(0);
  });

  test("onboarding: skip username step", async ({ freshPage }) => {
    // Walk through to the username step
    await expect(freshPage.getByText("Welcome to Variance")).toBeVisible({
      timeout: 10_000,
    });
    await freshPage.getByRole("button", { name: "Create new identity" }).click();
    await freshPage.getByRole("button", { name: "Continue without passphrase" }).click();
    await freshPage.getByRole("button", { name: "Generate my identity" }).click();
    await expect(freshPage.getByText("Your Recovery Phrase")).toBeVisible({ timeout: 10_000 });
    await freshPage.getByText("I have written down").click();
    await freshPage.getByRole("button", { name: "Continue" }).click();
    await expect(freshPage.getByText("Identity Ready")).toBeVisible();
    await freshPage.getByRole("button", { name: "Start Variance" }).click();

    // Username step — skip it
    await expect(freshPage.getByText("Choose a Username")).toBeVisible({
      timeout: 10_000,
    });
    await freshPage.getByRole("button", { name: "Skip for now" }).click();

    // Should land in the main app
    await expect(freshPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });
  });

  test("passphrase step: back button returns to welcome", async ({ freshPage }) => {
    await expect(freshPage.getByText("Welcome to Variance")).toBeVisible({
      timeout: 10_000,
    });
    await freshPage.getByRole("button", { name: "Create new identity" }).click();
    await expect(freshPage.getByText("Protect your identity")).toBeVisible();

    // Click back
    await freshPage.getByText("← Back").click();

    // Should be back at welcome
    await expect(freshPage.getByText("Welcome to Variance")).toBeVisible();
  });

  test("passphrase step: set passphrase with validation", async ({ freshPage }) => {
    await expect(freshPage.getByText("Welcome to Variance")).toBeVisible({
      timeout: 10_000,
    });
    await freshPage.getByRole("button", { name: "Create new identity" }).click();
    await expect(freshPage.getByText("Protect your identity")).toBeVisible();

    // "Set passphrase" button should be disabled initially (no passphrase entered)
    const setBtn = freshPage.getByRole("button", { name: "Set passphrase" });
    await expect(setBtn).toBeDisabled();

    // The passphrase inputs have no htmlFor/id association, so use CSS selectors.
    // First password input is the passphrase field, second is confirm.
    const passphraseInput = freshPage.locator('input[type="password"]').first();
    const confirmInput = freshPage.locator('input[type="password"]').nth(1);

    // Type a short passphrase
    await passphraseInput.fill("ab");
    await expect(freshPage.getByText("Too short", { exact: false })).toBeVisible();
    await expect(setBtn).toBeDisabled();

    // Type a valid passphrase — button enables even without confirm
    // (confirm field validation only triggers when confirm is non-empty)
    await passphraseInput.fill("testpass123");
    await expect(setBtn).toBeEnabled();

    // Confirm with mismatch — button disables
    await confirmInput.fill("wrong");
    await expect(freshPage.getByText("don't match", { exact: false })).toBeVisible();
    await expect(setBtn).toBeDisabled();

    // Fix the confirmation — button re-enables
    await confirmInput.fill("testpass123");
    await expect(setBtn).toBeEnabled();
  });
});

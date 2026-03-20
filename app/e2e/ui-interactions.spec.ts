import { test, expect } from "./fixtures";

test.describe("Create group via UI modal", () => {
  test("type group name, click Create, see group in sidebar and navigate to it", async ({
    appPage,
  }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Open the "New group" modal
    await appPage.getByTitle("New group").click();
    await expect(appPage.getByText("New Group", { exact: false })).toBeVisible();

    // The Create button should be disabled when the input is empty
    const createBtn = appPage.getByRole("button", { name: "Create", exact: true });
    await expect(createBtn).toBeDisabled();

    // Type a group name
    const groupNameInput = appPage.getByLabel("Group name");
    await groupNameInput.fill("UI Created Group");

    // Create button should now be enabled
    await expect(createBtn).toBeEnabled();

    // Click Create
    await createBtn.click();

    // The modal should close
    await expect(appPage.getByText("New Group", { exact: false })).not.toBeVisible({
      timeout: 5_000,
    });

    // The group should appear in the sidebar
    await expect(appPage.getByText("UI Created Group").first()).toBeVisible({ timeout: 5_000 });

    // The app should auto-navigate to the group view (placeholder disappears)
    await expect(appPage.getByText("Select a conversation", { exact: false })).not.toBeVisible({
      timeout: 5_000,
    });
  });

  test("empty group name keeps Create button disabled", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    await appPage.getByTitle("New group").click();
    await expect(appPage.getByText("New Group", { exact: false })).toBeVisible();

    const createBtn = appPage.getByRole("button", { name: "Create", exact: true });
    await expect(createBtn).toBeDisabled();

    // Type spaces only — should still be disabled (name.trim() is empty)
    const groupNameInput = appPage.getByLabel("Group name");
    await groupNameInput.fill("   ");
    await expect(createBtn).toBeDisabled();

    // Cancel closes modal
    await appPage.getByRole("button", { name: "Cancel" }).click();
    await expect(appPage.getByText("New Group", { exact: false })).not.toBeVisible();
  });
});

test.describe("Send group message via UI", () => {
  test("type a message in the editor, press Enter, see the message bubble appear", async ({
    appPage,
    apiPort,
  }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Create a group via API so we have a clean group to message in
    const res = await fetch(`http://127.0.0.1:${apiPort}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Message UI Test" }),
    });
    expect(res.ok).toBe(true);

    // Reload to see the group
    await appPage.reload();
    await expect(appPage.getByText("Message UI Test").first()).toBeVisible({ timeout: 10_000 });

    // Click the group to navigate to group view
    await appPage.getByText("Message UI Test").first().click();
    await expect(appPage.getByText("Select a conversation", { exact: false })).not.toBeVisible({
      timeout: 5_000,
    });

    // Wait for the message editor to be visible (signals the group view is loaded)
    const editor = appPage.locator(".ProseMirror").first();
    await expect(editor).toBeVisible({ timeout: 5_000 });

    // Use a unique message to avoid collisions with prior test runs
    const uniqueMsg = `Hello from Playwright ${Date.now()}`;

    // Focus the TipTap editor and type a message.
    await editor.click();
    await editor.pressSequentially(uniqueMsg);

    // Press Enter to send (MessageEditor sends on Enter without Shift)
    await appPage.keyboard.press("Enter");

    // The message bubble should appear with the sent text
    await expect(appPage.getByText(uniqueMsg).first()).toBeVisible({ timeout: 5_000 });
  });

  test("send button is disabled when editor is empty", async ({ appPage, apiPort }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Create and navigate to a group
    const res = await fetch(`http://127.0.0.1:${apiPort}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Send Btn Test" }),
    });
    expect(res.ok).toBe(true);

    await appPage.reload();
    await expect(appPage.getByText("Send Btn Test").first()).toBeVisible({ timeout: 10_000 });
    await appPage.getByText("Send Btn Test").first().click();

    // The send button (the button containing the Send icon) should be disabled
    // It's the only button inside the composer shell with disabled state
    const sendBtn = appPage.locator("button.bg-primary-500").first();
    await expect(sendBtn).toBeDisabled();

    // Type something — send button should become enabled
    const editor = appPage.locator(".ProseMirror").first();
    await editor.click();
    await editor.pressSequentially("a");
    await expect(sendBtn).toBeEnabled();
  });
});

test.describe("Settings overlay interactions", () => {
  test("displays identity DID and copy button works", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Open settings — Account section loads by default
    await appPage.getByTitle("Settings").first().click();
    await expect(appPage.getByRole("heading", { name: "Identity" })).toBeVisible();

    // DID should be visible
    await expect(appPage.getByText("did:variance:").first()).toBeVisible();

    // Copy button should be present — it says "Copy DID" (no username set yet)
    // or "Copy username" (if username was set by a prior test)
    const copyBtn = appPage.getByRole("button", { name: /Copy/ });
    await expect(copyBtn).toBeVisible();

    // Click the copy button
    await copyBtn.click();

    // After clicking, the button text should change to "Copied!"
    await expect(appPage.getByText("Copied!")).toBeVisible({ timeout: 2_000 });

    await appPage.getByTitle("Close settings").click();
  });

  test("retention dropdown changes value via custom Select", async ({ appPage, apiPort }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Remember current retention
    const retRes = await fetch(`http://127.0.0.1:${apiPort}/config/retention`);
    const original = (await retRes.json()) as { group_message_max_age_days: number };

    // Open settings and navigate to Storage section
    await appPage.getByTitle("Settings").first().click();
    await appPage.getByRole("button", { name: "Storage" }).click();
    await expect(appPage.getByText("Message Retention")).toBeVisible();

    // The custom Select has role="combobox" — click to open the dropdown
    const selectTrigger = appPage.getByRole("combobox");
    await expect(selectTrigger).toBeVisible();
    await selectTrigger.click();

    // The dropdown portal should appear with role="listbox"
    const listbox = appPage.getByRole("listbox");
    await expect(listbox).toBeVisible();

    // Click "14 days" option
    await listbox.getByRole("option", { name: "14 days" }).click();

    // Wait for the API call to complete
    await appPage.waitForTimeout(500);

    // Verify the backend was updated
    const verifyRes = await fetch(`http://127.0.0.1:${apiPort}/config/retention`);
    const updated = (await verifyRes.json()) as { group_message_max_age_days: number };
    expect(updated.group_message_max_age_days).toBe(14);

    // Restore original value
    await selectTrigger.click();
    const listbox2 = appPage.getByRole("listbox");
    await expect(listbox2).toBeVisible();

    // Find the matching option text for the original value
    const originalLabel =
      original.group_message_max_age_days === 0
        ? "Keep forever"
        : original.group_message_max_age_days === 90
          ? "90 days"
          : original.group_message_max_age_days === 14
            ? "14 days"
            : "30 days (default)";
    await listbox2.getByRole("option", { name: originalLabel }).click();

    await appPage.getByTitle("Close settings").click();
  });

  test("relay CRUD through the settings UI", async ({ appPage, apiPort }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Open settings and navigate to Network section
    await appPage.getByTitle("Settings").first().click();
    await appPage.getByRole("button", { name: "Network" }).click();
    await expect(appPage.getByRole("heading", { name: "Relay Servers" })).toBeVisible();

    // Fill in relay form
    const peerIdInput = appPage.getByPlaceholder("Peer ID");
    const multiaddrInput = appPage.getByPlaceholder("Multiaddr", { exact: false });
    await peerIdInput.fill("12D3KooWTestUIRelay1234567890123456789012345678");
    await multiaddrInput.fill("/ip4/10.0.0.1/tcp/4001");

    // Click "Add relay" — this auto-saves (no separate Save button)
    const addBtn = appPage.getByRole("button", { name: "Add relay" });
    await addBtn.click();

    // Wait for the API call to complete
    await appPage.waitForTimeout(500);

    // The relay should appear in the list
    await expect(
      appPage.getByText("12D3KooWTestUIRelay1234567890123456789012345678").first()
    ).toBeVisible();
    await expect(appPage.getByText("/ip4/10.0.0.1/tcp/4001").first()).toBeVisible();

    // The inputs should be cleared after adding
    await expect(peerIdInput).toHaveValue("");
    await expect(multiaddrInput).toHaveValue("");

    // Verify relay was saved on the backend
    const relayRes = await fetch(`http://127.0.0.1:${apiPort}/config/relays`);
    const relays = (await relayRes.json()) as { peer_id: string }[];
    const found = relays.find(
      (r) => r.peer_id === "12D3KooWTestUIRelay1234567890123456789012345678"
    );
    expect(found).toBeTruthy();

    // Remove the relay via the UI — click the remove button next to it
    const removeBtn = appPage.getByTitle("Remove relay").first();
    await removeBtn.click();

    // Wait for the API call to complete
    await appPage.waitForTimeout(500);

    // Verify relay was removed on the backend
    const verifyRes = await fetch(`http://127.0.0.1:${apiPort}/config/relays`);
    const afterRemove = (await verifyRes.json()) as { peer_id: string }[];
    const stillFound = afterRemove.find(
      (r) => r.peer_id === "12D3KooWTestUIRelay1234567890123456789012345678"
    );
    expect(stillFound).toBeFalsy();

    await appPage.getByTitle("Close settings").click();
  });

  test("Add relay button disabled when inputs empty", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Open settings and navigate to Network section
    await appPage.getByTitle("Settings").first().click();
    await appPage.getByRole("button", { name: "Network" }).click();
    await expect(appPage.getByRole("heading", { name: "Relay Servers" })).toBeVisible();

    const addBtn = appPage.getByRole("button", { name: "Add relay" });
    await expect(addBtn).toBeDisabled();

    // Fill only peer ID — still disabled
    await appPage.getByPlaceholder("Peer ID").fill("some-peer-id");
    await expect(addBtn).toBeDisabled();

    // Fill multiaddr too — now enabled
    await appPage.getByPlaceholder("Multiaddr", { exact: false }).fill("/ip4/1.2.3.4/tcp/4001");
    await expect(addBtn).toBeEnabled();

    await appPage.getByTitle("Close settings").click();
  });

  test("restore defaults shows confirmation dialog and removes all relays", async ({
    appPage,
    apiPort,
  }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // First, add a relay via the API so we have something to restore
    await fetch(`http://127.0.0.1:${apiPort}/config/relays`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        peer_id: "12D3KooWRestoreTest12345678901234567890123456",
        multiaddr: "/ip4/10.0.0.99/tcp/4001",
      }),
    });

    // Open settings and navigate to Network section
    await appPage.getByTitle("Settings").first().click();
    await appPage.getByRole("button", { name: "Network" }).click();
    await expect(appPage.getByRole("heading", { name: "Relay Servers" })).toBeVisible();

    // Wait for relay list to load
    await expect(
      appPage.getByText("12D3KooWRestoreTest12345678901234567890123456").first()
    ).toBeVisible({ timeout: 5_000 });

    // "Restore defaults" button should be visible when relays exist
    const restoreBtn = appPage.getByRole("button", { name: "Restore defaults" });
    await expect(restoreBtn).toBeVisible();
    await restoreBtn.click();

    // ConfirmDialog should appear
    await expect(appPage.getByRole("heading", { name: "Restore Defaults" })).toBeVisible();
    await expect(
      appPage.getByText("This will remove all configured relay servers")
    ).toBeVisible();

    // Click "Remove all" to confirm
    await appPage.getByRole("button", { name: "Remove all" }).click();

    // Wait for the operation to complete
    await appPage.waitForTimeout(500);

    // Relay should be gone
    await expect(
      appPage.getByText("12D3KooWRestoreTest12345678901234567890123456")
    ).not.toBeVisible({ timeout: 3_000 });

    // Verify on backend
    const verifyRes = await fetch(`http://127.0.0.1:${apiPort}/config/relays`);
    const afterRestore = (await verifyRes.json()) as { peer_id: string }[];
    expect(afterRestore.length).toBe(0);

    await appPage.getByTitle("Close settings").click();
  });
});

test.describe("Theme switching", () => {
  test("toggle dark mode via the theme toggle buttons", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // In Playwright (default light mode), the html element should not have data-theme="dark"
    const html = appPage.locator("html");
    await expect(html).not.toHaveAttribute("data-theme", "dark");

    // Click the Dark button (Moon icon)
    await appPage.getByTitle("Dark").click();

    // Now the html element should have data-theme="dark"
    await expect(html).toHaveAttribute("data-theme", "dark", { timeout: 2_000 });

    // Click the Light button (Sun icon)
    await appPage.getByTitle("Light").click();

    // The dark attribute should be removed
    await expect(html).not.toHaveAttribute("data-theme", "dark", { timeout: 2_000 });

    // Click System to restore default
    await appPage.getByTitle("System").click();
  });

  test("theme preference persists across reload", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Switch to dark mode
    await appPage.getByTitle("Dark").click();
    await expect(appPage.locator("html")).toHaveAttribute("data-theme", "dark", { timeout: 2_000 });

    // Reload the page
    await appPage.reload();
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Dark mode should persist
    await expect(appPage.locator("html")).toHaveAttribute("data-theme", "dark", { timeout: 2_000 });

    // Restore to system default
    await appPage.getByTitle("System").click();
    await expect(appPage.locator("html")).not.toHaveAttribute("data-theme", "dark", {
      timeout: 2_000,
    });
  });
});

test.describe("Quick-action popover", () => {
  test("popover opens from avatar click and shows copy DID action", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // The avatar/username area in the footer should be clickable.
    // It doesn't have a title, so we locate it by the avatar or "No username" text.
    // The footer contains the avatar button. Since the e2e identity has no username set,
    // it shows "No username".
    const avatarBtn = appPage.locator("button").filter({ hasText: /No username/ });
    // If a prior test set a username, fall back to the avatar area
    const hasNoUsername = (await avatarBtn.count()) > 0;

    if (hasNoUsername) {
      await avatarBtn.click();
    } else {
      // Username is set — the footer shows the display name next to the avatar
      // Click the first button in the footer that contains an avatar (img or svg)
      const footerAvatarBtn = appPage
        .locator(".border-t button")
        .first();
      await footerAvatarBtn.click();
    }

    // The popover should appear with "Copy DID"
    await expect(appPage.getByRole("button", { name: "Copy DID" })).toBeVisible({ timeout: 2_000 });

    // Click "Copy DID"
    await appPage.getByRole("button", { name: "Copy DID" }).click();

    // Should show "Copied!" feedback
    await expect(appPage.getByText("Copied!")).toBeVisible({ timeout: 2_000 });
  });

  test("popover closes on Escape", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Open the popover
    const footerBtn = appPage.locator(".border-t button").first();
    await footerBtn.click();
    await expect(appPage.getByRole("button", { name: "Copy DID" })).toBeVisible({ timeout: 2_000 });

    // Press Escape to close
    await appPage.keyboard.press("Escape");
    await expect(appPage.getByRole("button", { name: "Copy DID" })).not.toBeVisible();
  });

  test("popover closes on outside click", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Open the popover
    const footerBtn = appPage.locator(".border-t button").first();
    await footerBtn.click();
    await expect(appPage.getByRole("button", { name: "Copy DID" })).toBeVisible({ timeout: 2_000 });

    // Click somewhere outside (the main content area)
    await appPage.locator("main").click();
    await expect(appPage.getByRole("button", { name: "Copy DID" })).not.toBeVisible();
  });
});

test.describe("Appearance section theme cards", () => {
  test("theme cards switch theme from Appearance settings section", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    const html = appPage.locator("html");

    // Open settings and navigate to Appearance
    await appPage.getByTitle("Settings").first().click();
    await appPage.getByRole("button", { name: "Appearance" }).click();
    await expect(appPage.getByRole("heading", { name: "Theme" })).toBeVisible();

    // The three theme cards should be visible (Light, System, Dark)
    await expect(appPage.getByText("Light").first()).toBeVisible();
    await expect(appPage.getByText("System").first()).toBeVisible();
    await expect(appPage.getByText("Dark").first()).toBeVisible();

    // Click the "Dark" card
    await appPage.getByText("Always use dark theme").click();
    await expect(html).toHaveAttribute("data-theme", "dark", { timeout: 2_000 });

    // Should show "Currently using dark theme"
    await expect(appPage.getByText("Currently using dark theme")).toBeVisible();

    // Click the "Light" card
    await appPage.getByText("Always use light theme").click();
    await expect(html).not.toHaveAttribute("data-theme", "dark", { timeout: 2_000 });
    await expect(appPage.getByText("Currently using light theme")).toBeVisible();

    // Restore to system
    await appPage.getByText("Follow your OS setting").click();

    await appPage.getByTitle("Close settings").click();
  });
});

test.describe("Group view member sidebar", () => {
  test("toggle member sidebar open and closed", async ({ appPage, apiPort }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Create a group
    const res = await fetch(`http://127.0.0.1:${apiPort}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Sidebar Test Group" }),
    });
    expect(res.ok).toBe(true);

    await appPage.reload();
    await expect(appPage.getByText("Sidebar Test Group").first()).toBeVisible({
      timeout: 10_000,
    });
    await appPage.getByText("Sidebar Test Group").first().click();

    // Wait for the group view to load (header should show group name)
    await expect(
      appPage.locator("p").filter({ hasText: "Sidebar Test Group" }).first()
    ).toBeVisible({ timeout: 5_000 });

    // The member sidebar should be visible by default on wide screens.
    // It shows "Online — N" text.
    const onlineSection = appPage.getByText(/Online —/);
    await expect(onlineSection).toBeVisible({ timeout: 3_000 });

    // Click the "Hide members" button (Users icon in header) to close
    await appPage.getByTitle("Hide members").click();

    // The sidebar should be hidden
    await expect(onlineSection).not.toBeVisible({ timeout: 3_000 });

    // Click "Show members" to reopen
    await appPage.getByTitle("Show members").click();
    await expect(onlineSection).toBeVisible({ timeout: 3_000 });
  });

  test("member sidebar shows (you) label for local user", async ({ appPage, apiPort }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    // Create and navigate to a group
    const res = await fetch(`http://127.0.0.1:${apiPort}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "You Label Test" }),
    });
    expect(res.ok).toBe(true);

    await appPage.reload();
    await expect(appPage.getByText("You Label Test").first()).toBeVisible({ timeout: 10_000 });
    await appPage.getByText("You Label Test").first().click();

    // The member sidebar should show "(you)" next to the local user's name
    await expect(appPage.getByText("(you)")).toBeVisible({ timeout: 5_000 });
  });
});

test.describe("New conversation modal validation", () => {
  test("Start conversation button disabled with empty fields", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    await appPage.getByTitle("New conversation").click();
    await expect(appPage.getByText("New Conversation")).toBeVisible();

    // "Start conversation" button should be present but disabled
    const startBtn = appPage.getByRole("button", { name: "Start conversation" });
    await expect(startBtn).toBeDisabled();

    // Cancel closes the modal
    await appPage.getByRole("button", { name: "Cancel" }).click();
    await expect(appPage.getByText("New Conversation")).not.toBeVisible();
  });

  test("Start conversation button enables with valid DID input", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    await appPage.getByTitle("New conversation").click();
    await expect(appPage.getByText("New Conversation")).toBeVisible();

    const startBtn = appPage.getByRole("button", { name: "Start conversation" });
    const recipientInput = appPage.getByLabel("Recipient");

    // Type a valid DID
    await recipientInput.fill("did:variance:someuser");
    await expect(startBtn).toBeEnabled();

    // Clear the first message — button should become disabled
    const messageInput = appPage.getByLabel("First message");
    await messageInput.fill("");
    await expect(startBtn).toBeDisabled();

    // Restore message
    await messageInput.fill("Hello!");
    await expect(startBtn).toBeEnabled();

    // Type an invalid recipient (not a DID, not a username)
    await recipientInput.fill("123invalid");
    await expect(startBtn).toBeDisabled();

    await appPage.getByRole("button", { name: "Cancel" }).click();
  });

  test("shows error when starting conversation with unknown peer", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    await appPage.getByTitle("New conversation").click();
    await expect(appPage.getByText("New Conversation")).toBeVisible();

    const recipientInput = appPage.getByLabel("Recipient");
    await recipientInput.fill("did:variance:nonexistent");

    const startBtn = appPage.getByRole("button", { name: "Start conversation" });
    await startBtn.click();

    // Should show an error (422 SessionRequired from the backend)
    await expect(appPage.locator(".text-red-500").first()).toBeVisible({ timeout: 5_000 });

    await appPage.getByRole("button", { name: "Cancel" }).click();
  });

  test("first message field defaults to 'Hello!'", async ({ appPage }) => {
    await expect(appPage.getByText("Select a conversation", { exact: false })).toBeVisible({
      timeout: 10_000,
    });

    await appPage.getByTitle("New conversation").click();
    await expect(appPage.getByText("New Conversation")).toBeVisible();

    // The first message input should have a default value
    const messageInput = appPage.getByLabel("First message");
    await expect(messageInput).toHaveValue("Hello!");

    await appPage.getByRole("button", { name: "Cancel" }).click();
  });
});

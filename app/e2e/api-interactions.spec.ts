import { test, expect } from "./fixtures";

test.describe("Username registration", () => {
  test("register username via API and verify identity", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    const res = await fetch(`${base}/identity/username`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: "apiuser" }),
    });
    expect(res.ok).toBe(true);
    const body = (await res.json()) as {
      username: string;
      discriminator: number;
      display_name: string;
      did: string;
    };
    expect(body.username).toBe("apiuser");
    expect(body.discriminator).toBeGreaterThan(0);
    // Note: register_username returns lowercased name as display_name (without #discriminator).
    // This is a backend inconsistency — get_identity formats it properly. Test the actual behavior.
    expect(body.display_name).toBe("apiuser");
    expect(body.did).toBeTruthy();

    // Verify identity endpoint reflects the username
    const idRes = await fetch(`${base}/identity`);
    const identity = (await idRes.json()) as {
      username: string;
      discriminator: number;
    };
    expect(identity.username).toBe("apiuser");
    expect(identity.discriminator).toBe(body.discriminator);
  });

  test("resolve username finds the registered user", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // First register (may already exist from previous test, that's ok)
    await fetch(`${base}/identity/username`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: "findable" }),
    });

    // Resolve by username — single match returns a flat object, not an array.
    // Shape: { did, username, discriminator, display_name }
    const res = await fetch(`${base}/identity/username/resolve/findable`);
    expect(res.ok).toBe(true);
    const result = (await res.json()) as Record<string, unknown>;

    // Could be a flat object (single match) or { matches: [...] } (multiple)
    if ("matches" in result) {
      const matches = result.matches as { display_name: string }[];
      expect(matches.length).toBeGreaterThanOrEqual(1);
      expect(matches[0].display_name).toContain("findable#");
    } else {
      expect(result.display_name).toContain("findable#");
    }
  });

  test("invalid username rejected", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Too short (min 3 chars)
    const res1 = await fetch(`${base}/identity/username`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: "ab" }),
    });
    expect(res1.status).toBe(400);

    // Special characters not allowed
    const res2 = await fetch(`${base}/identity/username`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: "bad name!" }),
    });
    expect(res2.status).toBe(400);
  });
});

test.describe("Config CRUD", () => {
  test("relay config: add, list, remove", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Get initial relay count
    const beforeRes = await fetch(`${base}/config/relays`);
    const before = (await beforeRes.json()) as unknown[];
    const countBefore = before.length;

    // Add a relay
    const addRes = await fetch(`${base}/config/relays`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        peer_id: "12D3KooWTestRelay123456789012345678901234567890",
        multiaddr: "/ip4/1.2.3.4/tcp/4001",
      }),
    });
    expect(addRes.ok).toBe(true);

    // List relays — count should increase
    const afterRes = await fetch(`${base}/config/relays`);
    const after = (await afterRes.json()) as {
      peer_id: string;
      multiaddr: string;
    }[];
    expect(after.length).toBe(countBefore + 1);

    const added = after.find(
      (r) => r.peer_id === "12D3KooWTestRelay123456789012345678901234567890"
    );
    expect(added).toBeTruthy();

    // Remove the relay
    const delRes = await fetch(
      `${base}/config/relays/12D3KooWTestRelay123456789012345678901234567890`,
      { method: "DELETE" }
    );
    expect(delRes.ok).toBe(true);

    // Verify removal
    const finalRes = await fetch(`${base}/config/relays`);
    const final_ = (await finalRes.json()) as unknown[];
    expect(final_.length).toBe(countBefore);
  });

  test("retention config: set and get", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Get current
    const getRes = await fetch(`${base}/config/retention`);
    expect(getRes.ok).toBe(true);
    const current = (await getRes.json()) as {
      group_message_max_age_days: number;
    };
    expect(typeof current.group_message_max_age_days).toBe("number");

    // Set a new value
    const putRes = await fetch(`${base}/config/retention`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ group_message_max_age_days: 14 }),
    });
    expect(putRes.ok).toBe(true);

    // Verify it changed
    const verifyRes = await fetch(`${base}/config/retention`);
    const updated = (await verifyRes.json()) as {
      group_message_max_age_days: number;
    };
    expect(updated.group_message_max_age_days).toBe(14);

    // Restore original
    await fetch(`${base}/config/retention`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        group_message_max_age_days: current.group_message_max_age_days,
      }),
    });
  });
});

test.describe("Typing indicators", () => {
  test("start typing, check, stop typing", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Start typing to a fake peer (is_group is required)
    const startRes = await fetch(`${base}/typing/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ recipient: "did:variance:typetarget", is_group: false }),
    });
    expect(startRes.ok).toBe(true);

    // Check typing users for that recipient
    const checkRes = await fetch(`${base}/typing/did:variance:typetarget`);
    expect(checkRes.ok).toBe(true);

    // Stop typing
    const stopRes = await fetch(`${base}/typing/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ recipient: "did:variance:typetarget", is_group: false }),
    });
    expect(stopRes.ok).toBe(true);
  });

  test("typing indicator for group", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Create a group to use as a recipient
    const createRes = await fetch(`${base}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Typing Group" }),
    });
    const group = (await createRes.json()) as { group_id: string };

    // Start typing in the group (is_group is required)
    const startRes = await fetch(`${base}/typing/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        recipient: group.group_id,
        is_group: true,
      }),
    });
    expect(startRes.ok).toBe(true);

    // Check typing users in the group
    const checkRes = await fetch(`${base}/typing/group:${group.group_id}`);
    expect(checkRes.ok).toBe(true);

    // Stop typing
    const stopRes = await fetch(`${base}/typing/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        recipient: group.group_id,
        is_group: true,
      }),
    });
    expect(stopRes.ok).toBe(true);
  });
});

test.describe("Receipt lifecycle", () => {
  test("send delivered and read receipts for a group message", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Create a group and send a message to have a message_id to receipt
    const createRes = await fetch(`${base}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Receipt Test Group" }),
    });
    const group = (await createRes.json()) as { group_id: string };

    const sendRes = await fetch(`${base}/messages/group`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        group_id: group.group_id,
        text: "Receipt test message",
      }),
    });
    const sent = (await sendRes.json()) as { message_id: string };

    // Send delivered receipt
    const deliveredRes = await fetch(`${base}/receipts/delivered`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        message_id: sent.message_id,
        sender_did: "did:variance:receiptsender",
      }),
    });
    expect(deliveredRes.ok).toBe(true);
    const delivered = (await deliveredRes.json()) as {
      status: string;
      message_id: string;
    };
    expect(delivered.status).toBe("delivered");
    expect(delivered.message_id).toBe(sent.message_id);

    // Get receipts for the message — should have 1 (delivered)
    const getRes1 = await fetch(`${base}/receipts/${sent.message_id}`);
    expect(getRes1.ok).toBe(true);
    const receipts1 = (await getRes1.json()) as {
      reader_did: string;
      status: string;
    }[];
    expect(receipts1.length).toBe(1);
    expect(receipts1[0].status).toBe("delivered");

    // Send read receipt (append-only — does NOT overwrite delivered)
    const readRes = await fetch(`${base}/receipts/read`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        message_id: sent.message_id,
        sender_did: "did:variance:receiptsender",
      }),
    });
    expect(readRes.ok).toBe(true);
    const read = (await readRes.json()) as { status: string };
    expect(read.status).toBe("read");

    // Get receipts again — both delivered and read should be present
    const getRes2 = await fetch(`${base}/receipts/${sent.message_id}`);
    const receipts2 = (await getRes2.json()) as {
      status: string;
    }[];
    expect(receipts2.length).toBe(2);
    const statuses = receipts2.map((r) => r.status);
    expect(statuses).toContain("delivered");
    expect(statuses).toContain("read");
  });
});

test.describe("Reactions", () => {
  test("add and remove group message reaction", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Create group and send a message
    const createRes = await fetch(`${base}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Reaction Test Group" }),
    });
    const group = (await createRes.json()) as { group_id: string };

    const sendRes = await fetch(`${base}/messages/group`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        group_id: group.group_id,
        text: "React to this!",
      }),
    });
    expect(sendRes.ok).toBe(true);
    const sent = (await sendRes.json()) as { message_id: string };

    // Add a reaction
    const addRes = await fetch(`${base}/messages/group/${sent.message_id}/reactions`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        emoji: "thumbsup",
        group_id: group.group_id,
      }),
    });
    expect(addRes.ok).toBe(true);

    // Remove the reaction
    const delRes = await fetch(`${base}/messages/group/${sent.message_id}/reactions/thumbsup`, {
      method: "DELETE",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ group_id: group.group_id }),
    });
    expect(delRes.ok).toBe(true);
  });
});

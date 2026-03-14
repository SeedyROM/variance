import { test, expect } from "./fixtures";

test.describe("Messaging API", () => {
  test("group message: create group, send message, fetch messages", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Create a group
    const createRes = await fetch(`${base}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Msg Test Group" }),
    });
    expect(createRes.ok).toBe(true);
    const group = (await createRes.json()) as {
      group_id: string;
      name: string;
    };
    expect(group.group_id).toBeTruthy();

    // Send a message to the group
    const sendRes = await fetch(`${base}/messages/group`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        group_id: group.group_id,
        text: "Hello from e2e test!",
      }),
    });
    expect(sendRes.ok).toBe(true);
    const sent = (await sendRes.json()) as {
      message_id: string;
      success: boolean;
    };
    expect(sent.success).toBe(true);
    expect(sent.message_id).toBeTruthy();

    // Fetch messages for the group
    const fetchRes = await fetch(`${base}/messages/group/${group.group_id}`);
    expect(fetchRes.ok).toBe(true);
    const messages = (await fetchRes.json()) as {
      id: string;
      text: string;
    }[];
    expect(messages.length).toBeGreaterThanOrEqual(1);
    const found = messages.find((m) => m.id === sent.message_id);
    expect(found).toBeTruthy();
    expect(found!.text).toBe("Hello from e2e test!");
  });

  test("direct message: validation errors for bad input", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Missing DID
    const res1 = await fetch(`${base}/conversations`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ recipient_did: "", text: "Hello" }),
    });
    expect(res1.status).toBe(400);

    // Invalid DID (no "did:" prefix)
    const res2 = await fetch(`${base}/conversations`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ recipient_did: "notadid", text: "Hello" }),
    });
    expect(res2.status).toBe(400);

    // Empty text
    const res3 = await fetch(`${base}/conversations`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        recipient_did: "did:variance:bob",
        text: "",
      }),
    });
    expect(res3.status).toBe(400);

    // Text too long (> 4096 chars)
    const res4 = await fetch(`${base}/conversations`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        recipient_did: "did:variance:bob",
        text: "x".repeat(4097),
      }),
    });
    expect(res4.status).toBe(400);
  });

  test("direct message: session required for unknown peer", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Trying to start a conversation with an unknown peer without keys
    // should return 422 (SessionRequired)
    const res = await fetch(`${base}/conversations`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        recipient_did: "did:variance:unknownpeer",
        text: "Hello",
      }),
    });
    expect(res.status).toBe(422);
  });

  test("group message: send to nonexistent group fails", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    const res = await fetch(`${base}/messages/group`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        group_id: "nonexistent-group-id",
        text: "This should fail",
      }),
    });
    // Backend returns an error for nonexistent group
    expect(res.ok).toBe(false);
  });

  test("group message: empty text rejected", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    // Create a group first
    const createRes = await fetch(`${base}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Empty Msg Test" }),
    });
    const group = (await createRes.json()) as { group_id: string };

    const res = await fetch(`${base}/messages/group`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        group_id: group.group_id,
        text: "",
      }),
    });
    expect(res.status).toBe(400);
  });

  test("group message: text too long rejected", async ({ apiPort }) => {
    const base = `http://127.0.0.1:${apiPort}`;

    const createRes = await fetch(`${base}/mls/groups`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "Long Msg Test" }),
    });
    const group = (await createRes.json()) as { group_id: string };

    const res = await fetch(`${base}/messages/group`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        group_id: group.group_id,
        text: "x".repeat(4097),
      }),
    });
    expect(res.status).toBe(400);
  });
});

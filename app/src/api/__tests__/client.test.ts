import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  healthApi,
  identityApi,
  conversationsApi,
  messagesApi,
  groupsApi,
  invitationsApi,
  typingApi,
  receiptsApi,
  presenceApi,
  configApi,
  resetApiBase,
} from "../client";

// Mock Tauri invoke — returns port 9000
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(9000),
}));

// Track all fetch calls
let fetchCalls: { url: string; init?: RequestInit }[] = [];

function mockFetch(body: unknown, status = 200) {
  return vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? "OK" : "Bad Request",
    json: () => Promise.resolve(body),
  });
}

beforeEach(() => {
  fetchCalls = [];
  resetApiBase();
  // Default: successful empty response
  const fetchFn = mockFetch({});
  vi.stubGlobal("fetch", (...args: [string, RequestInit?]) => {
    fetchCalls.push({ url: args[0], init: args[1] });
    return fetchFn(...args);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("request internals", () => {
  it("caches API base URL after first call", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    await healthApi.check();
    await healthApi.check();
    // invoke should only be called once — second request uses cache
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("resets cached base URL when resetApiBase is called", async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    const callsBefore = vi.mocked(invoke).mock.calls.length;
    await healthApi.check();
    resetApiBase();
    await healthApi.check();
    // Two new invoke calls: one for each check() after cache was cleared
    const callsAfter = vi.mocked(invoke).mock.calls.length;
    expect(callsAfter - callsBefore).toBe(2);
  });

  it("throws when node is not running (port is null)", async () => {
    const mod = await import("@tauri-apps/api/core");
    vi.mocked(mod.invoke).mockResolvedValueOnce(null);
    resetApiBase();
    await expect(healthApi.check()).rejects.toThrow("Node is not running");
  });

  it("sends Content-Type application/json header", async () => {
    await healthApi.check();
    expect(fetchCalls[0].init?.headers).toEqual(
      expect.objectContaining({ "Content-Type": "application/json" })
    );
  });

  it("throws with error message from non-ok response body", async () => {
    vi.stubGlobal("fetch", mockFetch({ error: "not found" }, 404));
    resetApiBase();
    await expect(healthApi.check()).rejects.toThrow("not found");
  });

  it("falls back to statusText when response body has no error field", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: false,
        status: 500,
        statusText: "Internal Server Error",
        json: () => Promise.reject(new Error("bad json")),
      })
    );
    resetApiBase();
    await expect(healthApi.check()).rejects.toThrow("Internal Server Error");
  });
});

describe("healthApi", () => {
  it("calls GET /health", async () => {
    await healthApi.check();
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/health");
  });
});

describe("identityApi", () => {
  it("get calls GET /identity", async () => {
    await identityApi.get();
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/identity");
  });

  it("resolve encodes DID in URL", async () => {
    await identityApi.resolve("did:variance:abc");
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/identity/resolve/did%3Avariance%3Aabc");
  });

  it("registerUsername sends POST with body", async () => {
    await identityApi.registerUsername("alice");
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/identity/username");
    expect(fetchCalls[0].init?.method).toBe("POST");
    expect(JSON.parse(fetchCalls[0].init?.body as string)).toEqual({ username: "alice" });
  });

  it("resolveUsername encodes username in URL", async () => {
    await identityApi.resolveUsername("alice");
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/identity/username/resolve/alice");
  });
});

describe("conversationsApi", () => {
  it("list calls GET /conversations", async () => {
    await conversationsApi.list();
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/conversations");
  });

  it("start sends POST with body", async () => {
    const body = { peer_did: "did:x", text: "hello" };
    await conversationsApi.start(body as never);
    expect(fetchCalls[0].init?.method).toBe("POST");
    expect(JSON.parse(fetchCalls[0].init?.body as string)).toEqual(body);
  });

  it("delete sends DELETE with encoded DID", async () => {
    await conversationsApi.delete("did:variance:abc");
    expect(fetchCalls[0].init?.method).toBe("DELETE");
    expect(fetchCalls[0].url).toContain("did%3Avariance%3Aabc");
  });
});

describe("messagesApi", () => {
  it("getDirect without pagination", async () => {
    await messagesApi.getDirect("did:variance:peer");
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/messages/direct/did%3Avariance%3Apeer");
  });

  it("getDirect with before and limit", async () => {
    await messagesApi.getDirect("did:variance:peer", 1000, 50);
    expect(fetchCalls[0].url).toContain("before=1000");
    expect(fetchCalls[0].url).toContain("limit=50");
  });

  it("sendDirect sends POST", async () => {
    await messagesApi.sendDirect({ peer_did: "did:x", text: "hi" } as never);
    expect(fetchCalls[0].init?.method).toBe("POST");
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/messages/direct");
  });

  it("getGroup encodes group ID", async () => {
    await messagesApi.getGroup("group-123");
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/messages/group/group-123");
  });
});

describe("groupsApi", () => {
  it("list calls GET /mls/groups", async () => {
    await groupsApi.list();
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/mls/groups");
  });

  it("create sends POST with name", async () => {
    await groupsApi.create("My Group");
    expect(fetchCalls[0].init?.method).toBe("POST");
    expect(JSON.parse(fetchCalls[0].init?.body as string)).toEqual({ name: "My Group" });
  });

  it("invite sends POST with invitee", async () => {
    await groupsApi.invite("g1", "did:invitee");
    expect(fetchCalls[0].url).toContain("/mls/groups/g1/invite");
    expect(JSON.parse(fetchCalls[0].init?.body as string)).toEqual({ invitee: "did:invitee" });
  });

  it("leave sends POST", async () => {
    await groupsApi.leave("g1");
    expect(fetchCalls[0].url).toContain("/mls/groups/g1/leave");
    expect(fetchCalls[0].init?.method).toBe("POST");
  });

  it("delete sends DELETE", async () => {
    await groupsApi.delete("g1");
    expect(fetchCalls[0].init?.method).toBe("DELETE");
  });

  it("removeMember sends DELETE with encoded DIDs", async () => {
    await groupsApi.removeMember("g1", "did:variance:member");
    expect(fetchCalls[0].init?.method).toBe("DELETE");
    expect(fetchCalls[0].url).toContain("did%3Avariance%3Amember");
  });

  it("changeRole sends PUT with new_role", async () => {
    await groupsApi.changeRole("g1", "did:m", "admin");
    expect(fetchCalls[0].init?.method).toBe("PUT");
    expect(JSON.parse(fetchCalls[0].init?.body as string)).toEqual({ new_role: "admin" });
  });
});

describe("invitationsApi", () => {
  it("list calls GET /invitations", async () => {
    await invitationsApi.list();
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/invitations");
  });

  it("accept sends POST", async () => {
    await invitationsApi.accept("g1");
    expect(fetchCalls[0].url).toContain("/invitations/g1/accept");
    expect(fetchCalls[0].init?.method).toBe("POST");
  });

  it("decline sends POST", async () => {
    await invitationsApi.decline("g1");
    expect(fetchCalls[0].url).toContain("/invitations/g1/decline");
    expect(fetchCalls[0].init?.method).toBe("POST");
  });
});

describe("typingApi", () => {
  it("start sends POST to /typing/start", async () => {
    await typingApi.start({ recipient: "did:x" } as never);
    expect(fetchCalls[0].url).toContain("/typing/start");
    expect(fetchCalls[0].init?.method).toBe("POST");
  });

  it("stop sends POST to /typing/stop", async () => {
    await typingApi.stop({ recipient: "did:x" } as never);
    expect(fetchCalls[0].url).toContain("/typing/stop");
  });

  it("get encodes recipient in URL", async () => {
    await typingApi.get("did:variance:abc");
    expect(fetchCalls[0].url).toContain("did%3Avariance%3Aabc");
  });
});

describe("receiptsApi", () => {
  it("sendRead sends POST to /receipts/read", async () => {
    await receiptsApi.sendRead("msg-1", "did:sender");
    expect(fetchCalls[0].url).toContain("/receipts/read");
    expect(JSON.parse(fetchCalls[0].init?.body as string)).toEqual({
      message_id: "msg-1",
      sender_did: "did:sender",
    });
  });

  it("sendDelivered sends POST to /receipts/delivered", async () => {
    await receiptsApi.sendDelivered("msg-1", "did:sender");
    expect(fetchCalls[0].url).toContain("/receipts/delivered");
  });
});

describe("presenceApi", () => {
  it("get calls GET /presence", async () => {
    await presenceApi.get();
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/presence");
  });
});

describe("configApi", () => {
  it("getRelays calls GET /config/relays", async () => {
    await configApi.getRelays();
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/config/relays");
  });

  it("addRelay sends POST", async () => {
    await configApi.addRelay({ peer_id: "p1", address: "/ip4/1.2.3.4/tcp/4001" } as never);
    expect(fetchCalls[0].init?.method).toBe("POST");
  });

  it("removeRelay sends DELETE with encoded peer ID", async () => {
    await configApi.removeRelay("peer-123");
    expect(fetchCalls[0].init?.method).toBe("DELETE");
    expect(fetchCalls[0].url).toContain("peer-123");
  });

  it("getRetention calls GET /config/retention", async () => {
    await configApi.getRetention();
    expect(fetchCalls[0].url).toBe("http://127.0.0.1:9000/config/retention");
  });

  it("setRetention sends PUT", async () => {
    await configApi.setRetention({ group_message_max_age_days: 90 } as never);
    expect(fetchCalls[0].init?.method).toBe("PUT");
    expect(JSON.parse(fetchCalls[0].init?.body as string)).toEqual({
      group_message_max_age_days: 90,
    });
  });
});

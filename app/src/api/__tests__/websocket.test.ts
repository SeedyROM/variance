import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { VarianceWebSocket } from "../websocket";

// Mock @tauri-apps/api/core so tests run outside Tauri
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(9000),
}));

// Minimal WebSocket mock — must be a real class (not arrow fn) to work as a constructor
class MockWebSocket {
  static OPEN = 1;
  readyState = MockWebSocket.OPEN;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];

  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  constructor(_url: string) {}

  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3; // CLOSED
  }
}

let mockWs: MockWebSocket;

beforeEach(() => {
  mockWs = new MockWebSocket("ws://unused");
  vi.stubGlobal("WebSocket", function MockWsFactory(url: string) {
    mockWs = new MockWebSocket(url);
    return mockWs;
  });
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis.WebSocket as any).OPEN = 1;
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("VarianceWebSocket", () => {
  it("logs a warning on malformed message, does not throw", async () => {
    const ws = new VarianceWebSocket();
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

    await ws.connect();
    mockWs.onopen?.();
    mockWs.onmessage?.({ data: "not valid json {{{" });

    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringContaining("[WebSocket]"),
      "not valid json {{{",
      expect.anything()
    );
    ws.disconnect();
  });

  it("calls registered handler with flattened event", async () => {
    const ws = new VarianceWebSocket();
    const handler = vi.fn();
    ws.on(handler);

    await ws.connect();
    mockWs.onopen?.();
    mockWs.onmessage?.({ data: JSON.stringify({ type: "PeerOnline", data: { did: "did:x" } }) });

    expect(handler).toHaveBeenCalledWith({ type: "PeerOnline", did: "did:x" });
    ws.disconnect();
  });

  it("schedules reconnect after connection closes", async () => {
    vi.useFakeTimers();
    const ws = new VarianceWebSocket();
    await ws.connect();
    mockWs.onopen?.();
    mockWs.onclose?.();

    // reconnect timer should be pending
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    ws.disconnect();
    vi.useRealTimers();
  });

  it("fires WsConnected event on open", async () => {
    const ws = new VarianceWebSocket();
    const handler = vi.fn();
    ws.on(handler);

    await ws.connect();
    mockWs.onopen?.();

    expect(handler).toHaveBeenCalledWith({ type: "WsConnected" });
    ws.disconnect();
  });

  it("fires WsDisconnected event on close", async () => {
    const ws = new VarianceWebSocket();
    const handler = vi.fn();
    ws.on(handler);

    await ws.connect();
    mockWs.onopen?.();
    mockWs.onclose?.();

    expect(handler).toHaveBeenCalledWith({ type: "WsDisconnected" });
    ws.disconnect();
  });

  it("unsubscribes handler via returned function", async () => {
    const ws = new VarianceWebSocket();
    const handler = vi.fn();
    const unsub = ws.on(handler);

    unsub();

    await ws.connect();
    mockWs.onopen?.();
    mockWs.onmessage?.({ data: JSON.stringify({ type: "Test", data: {} }) });

    // WsConnected would have been called if still subscribed
    expect(handler).not.toHaveBeenCalled();
    ws.disconnect();
  });

  it("flattens event without data field", async () => {
    const ws = new VarianceWebSocket();
    const handler = vi.fn();
    ws.on(handler);

    await ws.connect();
    mockWs.onopen?.();
    mockWs.onmessage?.({ data: JSON.stringify({ type: "Ping" }) });

    // Should get { type: "Ping" } with no extra fields
    expect(handler).toHaveBeenCalledWith({ type: "Ping" });
    ws.disconnect();
  });

  it("does not reconnect after disconnect()", async () => {
    vi.useFakeTimers();
    const ws = new VarianceWebSocket();
    await ws.connect();
    mockWs.onopen?.();
    ws.disconnect();

    // No reconnect timer should remain
    expect(vi.getTimerCount()).toBe(0);
    vi.useRealTimers();
  });

  it("schedules reconnect when port is null", async () => {
    vi.useFakeTimers();
    const mod = await import("@tauri-apps/api/core");
    vi.mocked(mod.invoke).mockResolvedValueOnce(null);

    const ws = new VarianceWebSocket();
    await ws.connect();

    // Should have a reconnect timer pending
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    ws.disconnect();
    vi.useRealTimers();
  });

  it("supports multiple handlers simultaneously", async () => {
    const ws = new VarianceWebSocket();
    const h1 = vi.fn();
    const h2 = vi.fn();
    ws.on(h1);
    ws.on(h2);

    await ws.connect();
    mockWs.onopen?.();
    mockWs.onmessage?.({ data: JSON.stringify({ type: "Test", data: { x: 1 } }) });

    expect(h1).toHaveBeenCalledWith({ type: "Test", x: 1 });
    expect(h2).toHaveBeenCalledWith({ type: "Test", x: 1 });
    ws.disconnect();
  });

  it("onerror triggers close", async () => {
    const ws = new VarianceWebSocket();
    await ws.connect();
    const closeSpy = vi.spyOn(mockWs, "close");
    mockWs.onerror?.();
    expect(closeSpy).toHaveBeenCalled();
    ws.disconnect();
  });
});

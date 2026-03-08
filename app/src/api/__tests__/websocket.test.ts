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
});

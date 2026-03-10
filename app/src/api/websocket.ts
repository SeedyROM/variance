import { invoke } from "@tauri-apps/api/core";
import type { WsEvent } from "./types";

type EventHandler = (event: WsEvent) => void;

const INITIAL_RECONNECT_DELAY = 500;
const MAX_RECONNECT_DELAY = 30_000;

const HEARTBEAT_INTERVAL = 30_000;

export class VarianceWebSocket {
  private ws: WebSocket | null = null;
  private handlers: Set<EventHandler> = new Set();
  private reconnectDelay = INITIAL_RECONNECT_DELAY;
  private stopped = false;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;

  on(handler: EventHandler): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }

  async connect() {
    this.stopped = false;
    await this.attemptConnect();
  }

  disconnect() {
    this.stopped = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.clearHeartbeat();
    this.ws?.close();
    this.ws = null;
  }

  private clearHeartbeat() {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  private async attemptConnect() {
    let port: number | null;
    try {
      port = await invoke<number | null>("get_api_port");
    } catch {
      port = null;
    }

    if (!port) {
      this.scheduleReconnect();
      return;
    }

    const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`);
    this.ws = ws;

    ws.onopen = () => {
      this.reconnectDelay = INITIAL_RECONNECT_DELAY;
      this.handlers.forEach((h) => h({ type: "WsConnected" } as never));
      this.heartbeatTimer = setInterval(() => {
        if (this.ws?.readyState === WebSocket.OPEN) {
          this.ws.send(JSON.stringify({ type: "Ping" }));
        }
      }, HEARTBEAT_INTERVAL);
    };

    ws.onmessage = (e) => {
      try {
        const raw = JSON.parse(e.data as string) as {
          type: string;
          data?: Record<string, unknown>;
        };
        // Backend uses serde adjacently-tagged enums: { "type": "...", "data": { ... } }
        // Flatten into { type, ...data } so the rest of the app can access fields directly.
        const event = { type: raw.type, ...(raw.data ?? {}) } as WsEvent;
        this.handlers.forEach((h) => h(event));
      } catch (err) {
        console.warn("[WebSocket] Failed to parse message:", e.data, err);
      }
    };

    ws.onclose = () => {
      this.clearHeartbeat();
      this.ws = null;
      this.handlers.forEach((h) => h({ type: "WsDisconnected" } as never));
      if (!this.stopped) this.scheduleReconnect();
    };

    ws.onerror = () => {
      ws.close();
    };
  }

  private scheduleReconnect() {
    if (this.stopped) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectDelay = Math.min(this.reconnectDelay * 2, MAX_RECONNECT_DELAY);
      void this.attemptConnect();
    }, this.reconnectDelay);
  }
}

export const variantWs = new VarianceWebSocket();

import { invoke } from "@tauri-apps/api/core";
import type {
  Conversation,
  DirectMessage,
  GroupMessage,
  HealthResponse,
  IdentityStatus,
  MessageResponse,
  RegisterUsernameResponse,
  ResolvedIdentity,
  ResolvedUsername,
  ResolvedUsernameMultiple,
  SendDirectMessageRequest,
  StartConversationRequest,
  StartConversationResponse,
  TypingRequest,
  TypingUsers,
} from "./types";

// Module-level cache for the API base URL. Avoids re-invoking Tauri on every request.
let _apiBase: string | null = null;

async function getApiBase(): Promise<string> {
  if (_apiBase) return _apiBase;
  const port = await invoke<number | null>("get_api_port");
  if (!port) throw new Error("Node is not running");
  _apiBase = `http://127.0.0.1:${port}`;
  return _apiBase;
}

/** Reset the cached API base (call after node restart). */
export function resetApiBase() {
  _apiBase = null;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const base = await getApiBase();
  const res = await fetch(`${base}${path}`, {
    headers: { "Content-Type": "application/json", ...init?.headers },
    ...init,
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error((err as { error?: string }).error ?? res.statusText);
  }
  return res.json() as Promise<T>;
}

// ===== Health =====

export const healthApi = {
  check: () => request<HealthResponse>("/health"),
};

// ===== Identity =====

export const identityApi = {
  get: () => request<IdentityStatus>("/identity"),
  resolve: (did: string) =>
    request<ResolvedIdentity>(`/identity/resolve/${encodeURIComponent(did)}`),
  registerUsername: (username: string) =>
    request<RegisterUsernameResponse>("/identity/username", {
      method: "POST",
      body: JSON.stringify({ username }),
    }),
  resolveUsername: (username: string) =>
    request<ResolvedUsername | ResolvedUsernameMultiple>(
      `/identity/username/resolve/${encodeURIComponent(username)}`
    ),
};

// ===== Conversations =====

export const conversationsApi = {
  list: () => request<Conversation[]>("/conversations"),

  start: (body: StartConversationRequest) =>
    request<StartConversationResponse>("/conversations", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  delete: (peerDid: string) =>
    request<{ success: boolean }>(`/conversations/${encodeURIComponent(peerDid)}`, {
      method: "DELETE",
    }),
};

// ===== Messages =====

export const messagesApi = {
  getDirect: (peerDid: string, before?: number) => {
    const qs = before !== undefined ? `?before=${before}` : "";
    return request<DirectMessage[]>(`/messages/direct/${encodeURIComponent(peerDid)}${qs}`);
  },

  sendDirect: (body: SendDirectMessageRequest) =>
    request<MessageResponse>("/messages/direct", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  getGroup: (groupId: string) =>
    request<GroupMessage[]>(`/messages/group/${encodeURIComponent(groupId)}`),
};

// ===== Typing =====

export const typingApi = {
  start: (body: TypingRequest) =>
    request("/typing/start", { method: "POST", body: JSON.stringify(body) }),

  stop: (body: TypingRequest) =>
    request("/typing/stop", { method: "POST", body: JSON.stringify(body) }),

  get: (recipient: string) => request<TypingUsers>(`/typing/${encodeURIComponent(recipient)}`),
};

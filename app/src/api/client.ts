import { invoke } from "@tauri-apps/api/core";
import type {
  Conversation,
  DirectMessage,
  GroupMessage,
  HealthResponse,
  IdentityStatus,
  MessageResponse,
  MlsGroupInfo,
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

// ===== Groups =====

export const groupsApi = {
  list: () => request<MlsGroupInfo[]>("/mls/groups"),

  create: (name: string) =>
    request<{ success: boolean; group_id: string; name: string }>("/mls/groups", {
      method: "POST",
      body: JSON.stringify({ name }),
    }),

  getMessages: (groupId: string) =>
    request<GroupMessage[]>(`/messages/group/${encodeURIComponent(groupId)}`),

  sendMessage: (
    groupId: string,
    text: string,
    opts?: { reply_to?: string; metadata?: Record<string, string> }
  ) =>
    request<MessageResponse>("/messages/group", {
      method: "POST",
      body: JSON.stringify({ group_id: groupId, text, ...opts }),
    }),

  invite: (groupId: string, invitee: string) =>
    request<{ success: boolean; group_id: string; invitee_did: string }>(
      `/mls/groups/${encodeURIComponent(groupId)}/invite`,
      {
        method: "POST",
        body: JSON.stringify({ invitee }),
      }
    ),

  leave: (groupId: string) =>
    request<{ success: boolean }>(`/mls/groups/${encodeURIComponent(groupId)}/leave`, {
      method: "POST",
    }),
};

// ===== Reactions =====

export const reactionsApi = {
  add: (messageId: string, emoji: string, recipientDid: string) =>
    request<MessageResponse>(`/messages/direct/${encodeURIComponent(messageId)}/reactions`, {
      method: "POST",
      body: JSON.stringify({ emoji, recipient_did: recipientDid }),
    }),
  remove: (messageId: string, emoji: string, recipientDid: string) =>
    request<MessageResponse>(
      `/messages/direct/${encodeURIComponent(messageId)}/reactions/${encodeURIComponent(emoji)}?recipient_did=${encodeURIComponent(recipientDid)}`,
      { method: "DELETE" }
    ),

  addGroup: (messageId: string, emoji: string, groupId: string) =>
    request<MessageResponse>(`/messages/group/${encodeURIComponent(messageId)}/reactions`, {
      method: "POST",
      body: JSON.stringify({ emoji, group_id: groupId }),
    }),
  removeGroup: (messageId: string, emoji: string, groupId: string) =>
    request<MessageResponse>(
      `/messages/group/${encodeURIComponent(messageId)}/reactions/${encodeURIComponent(emoji)}`,
      { method: "DELETE", body: JSON.stringify({ group_id: groupId }) }
    ),
};

// ===== Typing =====

export const typingApi = {
  start: (body: TypingRequest) =>
    request("/typing/start", { method: "POST", body: JSON.stringify(body) }),

  stop: (body: TypingRequest) =>
    request("/typing/stop", { method: "POST", body: JSON.stringify(body) }),

  get: (recipient: string) => request<TypingUsers>(`/typing/${encodeURIComponent(recipient)}`),
};

// ===== Presence =====

export const presenceApi = {
  /** Get the list of currently connected peer DIDs. */
  get: () => request<{ online: string[] }>("/presence"),
};

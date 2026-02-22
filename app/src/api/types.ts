// ===== Identity =====

export interface IdentityStatus {
  did: string;
  verifying_key: string;
  created_at: string;
  olm_identity_key: string;
  one_time_keys: string[];
  username?: string;
  discriminator?: number;
  display_name?: string;
}

export interface ResolvedIdentity {
  did: string;
  verifying_key?: string;
  created_at?: string;
  resolved: boolean;
}

// ===== Conversations =====

export interface Conversation {
  id: string;
  peer_did: string;
  last_message_timestamp: number;
  peer_username?: string;
}

export interface StartConversationRequest {
  recipient_did: string;
  text: string;
  recipient_identity_key?: string;
  recipient_one_time_key?: string;
}

export interface StartConversationResponse {
  conversation_id: string;
  message_id: string;
}

// ===== Messages =====

export interface DirectMessage {
  id: string;
  sender_did: string;
  recipient_did: string;
  text: string;
  timestamp: number;
  reply_to?: string;
  status?: "sent" | "pending" | "failed";
  sender_username?: string;
}

export interface GroupMessage {
  id: string;
  sender_did: string;
  group_id: string;
  text: string;
  timestamp: number;
  reply_to?: string;
  sender_username?: string;
}

export interface SendDirectMessageRequest {
  recipient_did: string;
  text: string;
  reply_to?: string;
}

export interface MessageResponse {
  message_id: string;
  success: boolean;
  message: string;
}

// ===== Typing =====

export interface TypingRequest {
  recipient: string;
  is_group: boolean;
}

export interface TypingUsers {
  users: string[];
}

// ===== Health =====

export interface HealthResponse {
  status: string;
  service: string;
}

// ===== WebSocket Events =====

export type WsEvent =
  | {
      type: "DirectMessageReceived";
      from: string;
      message_id: string;
      text: string;
      timestamp: number;
      reply_to?: string;
    }
  | {
      type: "DirectMessageSent";
      recipient: string;
      message_id: string;
      text: string;
      timestamp: number;
      reply_to?: string;
    }
  | { type: "GroupMessageReceived"; group_id: string; message_id: string }
  | { type: "TypingStarted"; from: string; recipient: string }
  | { type: "TypingStopped"; from: string; recipient: string }
  | { type: "ReceiptDelivered"; message_id: string }
  | { type: "ReceiptRead"; message_id: string }
  | { type: "CallIncoming"; call_id: string; from: string; call_type: string }
  | { type: "CallEnded"; call_id: string }
  | { type: "PresenceUpdated"; did: string; online: boolean; display_name?: string };

// ===== Tauri Commands =====

export interface GeneratedIdentity {
  did: string;
  mnemonic: string[];
}

export interface NodeStatus {
  running: boolean;
  local_did: string | null;
  api_port: number | null;
}

// ===== Username Resolution =====

export interface RegisterUsernameResponse {
  username: string;
  discriminator: number;
  display_name: string;
  did: string;
}

export interface ResolvedUsername {
  did: string;
  username: string;
  discriminator: number;
  display_name: string;
}

export interface ResolvedUsernameMultiple {
  matches: ResolvedUsername[];
}

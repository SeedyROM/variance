import { create } from "zustand";

export type ActiveConversation =
  | { type: "dm"; peerId: string }
  | { type: "group"; groupId: string };

// Auto-expiry for typing indicators (ms). Slightly longer than the backend's
// 2s outbound cooldown so we don't flicker between renewals.
const TYPING_EXPIRY_MS = 3_000;

interface MessagingStore {
  activeConversation: ActiveConversation | null;
  setActiveConversation: (conv: ActiveConversation | null) => void;
  // Presence tracking: Map from DID to online status
  presenceMap: Map<string, boolean>;
  setPresence: (did: string, online: boolean) => void;
  /** Replace the entire presence map from a full list of online DIDs. */
  syncPresence: (onlineDids: string[]) => void;
  // Peer display names: Map from DID to display_name (e.g. "alice#0042")
  peerNames: Map<string, string>;
  setPeerName: (did: string, name: string) => void;
  // Unread tracking: Set of conversation/group IDs with unread messages
  unreadConversations: Set<string>;
  markUnread: (conversationId: string) => void;
  markRead: (conversationId: string) => void;
  // Typing indicators: Map from conversation key (peer DID or "group:{id}") to
  // the set of DIDs that are currently typing in that conversation.
  typingUsers: Map<string, Set<string>>;
  setTyping: (from: string, recipient: string, isTyping: boolean) => void;
}

// Timers for auto-expiring typing indicators, keyed by "{recipient}::{from}".
// Kept outside the store to avoid Zustand serialization issues.
const typingExpireTimers = new Map<string, ReturnType<typeof setTimeout>>();

export const useMessagingStore = create<MessagingStore>((set) => ({
  activeConversation: null,
  setActiveConversation: (activeConversation) =>
    set((s) => {
      // When switching away from a conversation, clear any stale typing indicators
      // and cancel their auto-expiry timers so they don't bleed into the new view.
      const newTypingUsers = new Map(s.typingUsers);
      if (s.activeConversation) {
        const oldKey =
          s.activeConversation.type === "dm"
            ? s.activeConversation.peerId
            : `group:${s.activeConversation.groupId}`;
        newTypingUsers.delete(oldKey);
        for (const [timerKey, timer] of typingExpireTimers) {
          if (timerKey.startsWith(`${oldKey}::`)) {
            clearTimeout(timer);
            typingExpireTimers.delete(timerKey);
          }
        }
      }
      return { activeConversation, typingUsers: newTypingUsers };
    }),
  presenceMap: new Map(),
  setPresence: (did, online) =>
    set((s) => {
      const newMap = new Map(s.presenceMap);
      newMap.set(did, online);
      return { presenceMap: newMap };
    }),
  syncPresence: (onlineDids) =>
    set((s) => {
      const onlineSet = new Set(onlineDids);
      const newMap = new Map<string, boolean>();
      for (const [did] of s.presenceMap) {
        newMap.set(did, onlineSet.has(did));
      }
      for (const did of onlineDids) {
        newMap.set(did, true);
      }
      return { presenceMap: newMap };
    }),
  peerNames: new Map(),
  setPeerName: (did, name) =>
    set((s) => {
      const newMap = new Map(s.peerNames);
      newMap.set(did, name);
      return { peerNames: newMap };
    }),
  unreadConversations: new Set(),
  markUnread: (conversationId) =>
    set((s) => {
      const newSet = new Set(s.unreadConversations);
      newSet.add(conversationId);
      return { unreadConversations: newSet };
    }),
  markRead: (conversationId) =>
    set((s) => {
      const newSet = new Set(s.unreadConversations);
      newSet.delete(conversationId);
      return { unreadConversations: newSet };
    }),
  typingUsers: new Map(),
  setTyping: (from, recipient, isTyping) => {
    const timerKey = `${recipient}::${from}`;

    // Always cancel any pending expiry for this (recipient, sender) pair.
    const existing = typingExpireTimers.get(timerKey);
    if (existing !== undefined) {
      clearTimeout(existing);
      typingExpireTimers.delete(timerKey);
    }

    if (isTyping) {
      // Schedule auto-expiry so indicators clear even when stop is suppressed
      // (groups) or the stop message is lost in transit.
      const timer = setTimeout(() => {
        typingExpireTimers.delete(timerKey);
        set((s) => {
          const newMap = new Map(s.typingUsers);
          const current = new Set(newMap.get(recipient) ?? []);
          current.delete(from);
          if (current.size === 0) {
            newMap.delete(recipient);
          } else {
            newMap.set(recipient, current);
          }
          return { typingUsers: newMap };
        });
      }, TYPING_EXPIRY_MS);
      typingExpireTimers.set(timerKey, timer);
    }

    set((s) => {
      const newMap = new Map(s.typingUsers);
      const current = new Set(newMap.get(recipient) ?? []);
      if (isTyping) {
        current.add(from);
      } else {
        current.delete(from);
      }
      if (current.size === 0) {
        newMap.delete(recipient);
      } else {
        newMap.set(recipient, current);
      }
      return { typingUsers: newMap };
    });
  },
}));

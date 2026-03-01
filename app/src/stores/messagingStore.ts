import { create } from "zustand";

export type ActiveConversation =
  | { type: "dm"; peerId: string }
  | { type: "group"; groupId: string };

interface MessagingStore {
  activeConversation: ActiveConversation | null;
  setActiveConversation: (conv: ActiveConversation | null) => void;
  // Monotonically incrementing counter — bumped on DirectMessageReceived.
  inboundMessageTick: number;
  tickInboundMessage: () => void;
  // Same pattern for group messages.
  groupMessageTick: number;
  tickGroupMessage: () => void;
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

export const useMessagingStore = create<MessagingStore>((set) => ({
  activeConversation: null,
  setActiveConversation: (activeConversation) => set({ activeConversation }),
  inboundMessageTick: 0,
  tickInboundMessage: () => set((s) => ({ inboundMessageTick: s.inboundMessageTick + 1 })),
  groupMessageTick: 0,
  tickGroupMessage: () => set((s) => ({ groupMessageTick: s.groupMessageTick + 1 })),
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
  setTyping: (from, recipient, isTyping) =>
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
    }),
}));

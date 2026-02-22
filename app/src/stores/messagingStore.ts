import { create } from "zustand";

interface MessagingStore {
  activeConversationId: string | null;
  setActiveConversationId: (id: string | null) => void;
  // Monotonically incrementing counter - bumped whenever a DirectMessageReceived
  // event arrives. MessageView watches this to trigger a refetch using its own
  // refetch() function, which avoids any dependency on DID string matching between
  // the WebSocket event and the React Query key.
  inboundMessageTick: number;
  tickInboundMessage: () => void;
  // Presence tracking: Map from DID to online status
  presenceMap: Map<string, boolean>;
  setPresence: (did: string, online: boolean) => void;
  /** Replace the entire presence map from a full list of online DIDs. */
  syncPresence: (onlineDids: string[]) => void;
  // Peer display names: Map from DID to display_name (e.g. "alice#0042")
  // Persisted from PresenceUpdated events and message sender_username fields.
  peerNames: Map<string, string>;
  setPeerName: (did: string, name: string) => void;
  // Unread tracking: Set of conversation IDs with unread messages
  unreadConversations: Set<string>;
  markUnread: (conversationId: string) => void;
  markRead: (conversationId: string) => void;
}

export const useMessagingStore = create<MessagingStore>((set) => ({
  activeConversationId: null,
  setActiveConversationId: (activeConversationId) => set({ activeConversationId }),
  inboundMessageTick: 0,
  tickInboundMessage: () => set((s) => ({ inboundMessageTick: s.inboundMessageTick + 1 })),
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
      // Mark everyone we've ever tracked: offline unless in the new list
      for (const [did] of s.presenceMap) {
        newMap.set(did, onlineSet.has(did));
      }
      // Add any new DIDs we haven't seen before
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
}));

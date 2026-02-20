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
}

export const useMessagingStore = create<MessagingStore>((set) => ({
  activeConversationId: null,
  setActiveConversationId: (activeConversationId) => set({ activeConversationId }),
  inboundMessageTick: 0,
  tickInboundMessage: () => set((s) => ({ inboundMessageTick: s.inboundMessageTick + 1 })),
}));

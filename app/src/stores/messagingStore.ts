import { create } from "zustand";

interface MessagingStore {
  activeConversationId: string | null;
  setActiveConversationId: (id: string | null) => void;
}

export const useMessagingStore = create<MessagingStore>((set) => ({
  activeConversationId: null,
  setActiveConversationId: (activeConversationId) => set({ activeConversationId }),
}));

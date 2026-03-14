import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import { useMessagingStore } from "../messagingStore";

beforeEach(() => {
  useMessagingStore.setState({
    activeConversation: null,
    presenceMap: new Map(),
    peerNames: new Map(),
    unreadConversations: new Set(),
    typingUsers: new Map(),
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("markRead / markUnread", () => {
  it("markUnread adds conversation to unreadConversations", () => {
    useMessagingStore.getState().markUnread("conv-1");
    expect(useMessagingStore.getState().unreadConversations.has("conv-1")).toBe(true);
  });

  it("markRead removes conversation from unreadConversations", () => {
    useMessagingStore.getState().markUnread("conv-1");
    useMessagingStore.getState().markRead("conv-1");
    expect(useMessagingStore.getState().unreadConversations.has("conv-1")).toBe(false);
  });
});

describe("setTyping", () => {
  it("adds DID to typing set when isTyping=true", () => {
    useMessagingStore.getState().setTyping("alice", "group:g1", true);
    const typingSet = useMessagingStore.getState().typingUsers.get("group:g1");
    expect(typingSet?.has("alice")).toBe(true);
  });

  it("removes DID from typing set when isTyping=false", () => {
    useMessagingStore.getState().setTyping("alice", "group:g1", true);
    useMessagingStore.getState().setTyping("alice", "group:g1", false);
    const typingSet = useMessagingStore.getState().typingUsers.get("group:g1");
    expect(typingSet?.has("alice")).toBeFalsy();
  });

  it("auto-expires typing indicator after 3s", () => {
    vi.useFakeTimers();
    useMessagingStore.getState().setTyping("alice", "group:g1", true);
    expect(useMessagingStore.getState().typingUsers.get("group:g1")?.has("alice")).toBe(true);
    vi.advanceTimersByTime(3_100);
    expect(useMessagingStore.getState().typingUsers.get("group:g1")?.has("alice")).toBeFalsy();
  });
});

describe("setPeerName", () => {
  it("stores the correct display name for a DID", () => {
    useMessagingStore.getState().setPeerName("did:variance:abc", "alice#0001");
    expect(useMessagingStore.getState().peerNames.get("did:variance:abc")).toBe("alice#0001");
  });
});

describe("setActiveConversation", () => {
  it("updates activeConversation", () => {
    useMessagingStore.getState().setActiveConversation({ type: "dm", peerId: "did:x" });
    expect(useMessagingStore.getState().activeConversation).toEqual({
      type: "dm",
      peerId: "did:x",
    });
  });

  it("clears activeConversation when set to null", () => {
    useMessagingStore.getState().setActiveConversation({ type: "dm", peerId: "did:x" });
    useMessagingStore.getState().setActiveConversation(null);
    expect(useMessagingStore.getState().activeConversation).toBeNull();
  });

  it("clears typing indicators for previous DM conversation on switch", () => {
    const { setActiveConversation, setTyping } = useMessagingStore.getState();
    setActiveConversation({ type: "dm", peerId: "did:alice" });
    setTyping("did:alice", "did:alice", true);
    expect(useMessagingStore.getState().typingUsers.get("did:alice")?.has("did:alice")).toBe(true);

    setActiveConversation({ type: "dm", peerId: "did:bob" });
    expect(useMessagingStore.getState().typingUsers.get("did:alice")).toBeUndefined();
  });

  it("clears typing indicators for previous group conversation on switch", () => {
    const { setActiveConversation, setTyping } = useMessagingStore.getState();
    setActiveConversation({ type: "group", groupId: "group-1" });
    setTyping("did:alice", "group:group-1", true);
    setTyping("did:bob", "group:group-1", true);
    expect(useMessagingStore.getState().typingUsers.get("group:group-1")?.size).toBe(2);

    setActiveConversation(null);
    expect(useMessagingStore.getState().typingUsers.get("group:group-1")).toBeUndefined();
  });
});

describe("setPresence", () => {
  it("marks a peer online", () => {
    useMessagingStore.getState().setPresence("did:variance:alice", true);
    expect(useMessagingStore.getState().presenceMap.get("did:variance:alice")).toBe(true);
  });

  it("marks a peer offline", () => {
    useMessagingStore.getState().setPresence("did:variance:alice", true);
    useMessagingStore.getState().setPresence("did:variance:alice", false);
    expect(useMessagingStore.getState().presenceMap.get("did:variance:alice")).toBe(false);
  });
});

describe("syncPresence", () => {
  it("replaces entire presence map from a list of online DIDs", () => {
    useMessagingStore.getState().setPresence("did:a", true);
    useMessagingStore.getState().setPresence("did:b", true);

    // Sync with only did:b online — did:a should become false
    useMessagingStore.getState().syncPresence(["did:b"]);
    const map = useMessagingStore.getState().presenceMap;
    expect(map.get("did:a")).toBe(false);
    expect(map.get("did:b")).toBe(true);
  });

  it("adds new peers from the online list", () => {
    useMessagingStore.getState().syncPresence(["did:new"]);
    expect(useMessagingStore.getState().presenceMap.get("did:new")).toBe(true);
  });

  it("handles empty online list — marks all known peers offline", () => {
    useMessagingStore.getState().setPresence("did:a", true);
    useMessagingStore.getState().syncPresence([]);
    expect(useMessagingStore.getState().presenceMap.get("did:a")).toBe(false);
  });
});

describe("pendingInvitationCount", () => {
  it("starts at zero", () => {
    expect(useMessagingStore.getState().pendingInvitationCount).toBe(0);
  });

  it("setPendingInvitationCount sets exact value", () => {
    useMessagingStore.getState().setPendingInvitationCount(5);
    expect(useMessagingStore.getState().pendingInvitationCount).toBe(5);
  });

  it("incrementPendingInvitations adds one", () => {
    useMessagingStore.getState().setPendingInvitationCount(3);
    useMessagingStore.getState().incrementPendingInvitations();
    expect(useMessagingStore.getState().pendingInvitationCount).toBe(4);
  });

  it("decrementPendingInvitations subtracts one", () => {
    useMessagingStore.getState().setPendingInvitationCount(3);
    useMessagingStore.getState().decrementPendingInvitations();
    expect(useMessagingStore.getState().pendingInvitationCount).toBe(2);
  });

  it("decrementPendingInvitations floors at zero", () => {
    useMessagingStore.getState().setPendingInvitationCount(0);
    useMessagingStore.getState().decrementPendingInvitations();
    expect(useMessagingStore.getState().pendingInvitationCount).toBe(0);
  });
});

describe("markRead / markUnread — multiple conversations", () => {
  it("tracks multiple unread conversations independently", () => {
    const store = useMessagingStore.getState();
    store.markUnread("conv-1");
    store.markUnread("conv-2");
    store.markRead("conv-1");
    const unread = useMessagingStore.getState().unreadConversations;
    expect(unread.has("conv-1")).toBe(false);
    expect(unread.has("conv-2")).toBe(true);
  });

  it("markRead on non-existent conversation is a no-op", () => {
    useMessagingStore.getState().markRead("nonexistent");
    expect(useMessagingStore.getState().unreadConversations.size).toBe(0);
  });
});

describe("setPeerName — overwrites", () => {
  it("overwrites existing display name", () => {
    useMessagingStore.getState().setPeerName("did:x", "old#0001");
    useMessagingStore.getState().setPeerName("did:x", "new#0002");
    expect(useMessagingStore.getState().peerNames.get("did:x")).toBe("new#0002");
  });
});

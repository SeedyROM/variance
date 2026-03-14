import { describe, it, expect } from "vitest";
import type { GroupMessage } from "../../../api/types";

/**
 * The filtering logic from GroupView that separates displayable messages
 * from control messages (reactions, role changes).  Extracted here so we
 * can test it without rendering the full component tree.
 */
function filterDisplayableMessages(messages: GroupMessage[]) {
  const reactionMessages = messages.filter((m) => m.metadata?.type === "reaction");
  const displayMessages = messages.filter(
    (m) => m.metadata?.type !== "reaction" && m.metadata?.type !== "role_change"
  );
  return { reactionMessages, displayMessages };
}

const baseMsg: GroupMessage = {
  id: "msg-1",
  sender_did: "did:variance:alice",
  group_id: "group-1",
  text: "Hello!",
  timestamp: 1000,
};

describe("group message filtering", () => {
  it("keeps regular messages", () => {
    const { displayMessages } = filterDisplayableMessages([baseMsg]);
    expect(displayMessages).toHaveLength(1);
    expect(displayMessages[0].text).toBe("Hello!");
  });

  it("filters out role_change messages", () => {
    const roleChangeMsg: GroupMessage = {
      ...baseMsg,
      id: "msg-role",
      text: "",
      metadata: {
        type: "role_change",
        target_did: "did:variance:bob",
        new_role: "moderator",
      },
    };
    const { displayMessages, reactionMessages } = filterDisplayableMessages([
      baseMsg,
      roleChangeMsg,
    ]);
    expect(displayMessages).toHaveLength(1);
    expect(displayMessages[0].id).toBe("msg-1");
    expect(reactionMessages).toHaveLength(0);
  });

  it("filters out reaction messages into their own list", () => {
    const reactionMsg: GroupMessage = {
      ...baseMsg,
      id: "msg-react",
      text: "",
      metadata: {
        type: "reaction",
        message_id: "msg-1",
        emoji: "thumbsup",
        action: "add",
      },
    };
    const { displayMessages, reactionMessages } = filterDisplayableMessages([baseMsg, reactionMsg]);
    expect(displayMessages).toHaveLength(1);
    expect(reactionMessages).toHaveLength(1);
    expect(reactionMessages[0].id).toBe("msg-react");
  });

  it("filters both role_change and reaction from a mixed list", () => {
    const msgs: GroupMessage[] = [
      { ...baseMsg, id: "msg-1", text: "First" },
      {
        ...baseMsg,
        id: "msg-role",
        text: "",
        metadata: { type: "role_change", target_did: "did:bob", new_role: "moderator" },
      },
      {
        ...baseMsg,
        id: "msg-react",
        text: "",
        metadata: { type: "reaction", message_id: "msg-1", emoji: "heart", action: "add" },
      },
      { ...baseMsg, id: "msg-2", text: "Second" },
    ];
    const { displayMessages, reactionMessages } = filterDisplayableMessages(msgs);
    expect(displayMessages).toHaveLength(2);
    expect(displayMessages.map((m) => m.id)).toEqual(["msg-1", "msg-2"]);
    expect(reactionMessages).toHaveLength(1);
  });

  it("handles messages with no metadata", () => {
    const noMetaMsg: GroupMessage = { ...baseMsg, metadata: undefined };
    const { displayMessages } = filterDisplayableMessages([noMetaMsg]);
    expect(displayMessages).toHaveLength(1);
  });

  it("handles empty metadata object", () => {
    const emptyMetaMsg: GroupMessage = { ...baseMsg, metadata: {} };
    const { displayMessages } = filterDisplayableMessages([emptyMetaMsg]);
    expect(displayMessages).toHaveLength(1);
  });

  it("returns empty lists for empty input", () => {
    const { displayMessages, reactionMessages } = filterDisplayableMessages([]);
    expect(displayMessages).toHaveLength(0);
    expect(reactionMessages).toHaveLength(0);
  });
});

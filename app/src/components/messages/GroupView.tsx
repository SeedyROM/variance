import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Send, User } from "lucide-react";
import { GroupHeader } from "./GroupHeader";
import { GroupMessageBubble } from "./GroupMessageBubble";
import { DateDivider } from "./DateDivider";
import { ScrollArea } from "../ui/ScrollArea";
import { messagesApi, groupsApi, reactionsApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useMessagingStore } from "../../stores/messagingStore";
import { isDifferentDay } from "../../utils/time";
import type { GroupMessage, ReactionSummary } from "../../api/types";

interface GroupViewProps {
  groupId: string;
}

function GroupMessageInput({ groupId }: { groupId: string }) {
  const [text, setText] = useState("");
  const queryClient = useQueryClient();

  const sendMutation = useMutation({
    mutationFn: () => groupsApi.sendMessage(groupId, text.trim()),
    onSuccess: () => {
      setText("");
      void queryClient.invalidateQueries({ queryKey: ["messages", "group", groupId] });
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
    },
  });

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (text.trim() && !sendMutation.isPending) sendMutation.mutate();
    }
  };

  return (
    <div className="border-t border-surface-200 bg-surface-50 px-4 py-3 dark:border-surface-800 dark:bg-surface-900">
      <div className="flex items-center gap-2 rounded-xl border border-surface-300 bg-white px-3 py-2 focus-within:border-primary-500 focus-within:ring-2 focus-within:ring-primary-500/20 dark:border-surface-700 dark:bg-surface-950">
        <input
          type="text"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Message group"
          className="flex-1 min-w-0 text-sm text-surface-900 dark:text-surface-50 bg-transparent focus:outline-none"
        />
        <button
          onClick={() => {
            if (text.trim() && !sendMutation.isPending) sendMutation.mutate();
          }}
          disabled={!text.trim() || sendMutation.isPending}
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-primary-500 text-white transition-colors hover:bg-primary-600 disabled:opacity-40"
        >
          <Send className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

function groupShowSenderAbove(
  messages: GroupMessage[],
  index: number,
  localDid: string | null
): boolean {
  const msg = messages[index];
  if (msg.sender_did === localDid) return false;
  if (index === 0) return true;
  const prev = messages[index - 1];
  return prev.sender_did !== msg.sender_did || isDifferentDay(prev.timestamp, msg.timestamp);
}

/** Squash reaction messages into per-message, per-emoji counts. */
function aggregateGroupReactions(
  reactionMsgs: GroupMessage[],
  localDid: string | null
): Map<string, ReactionSummary[]> {
  const byMessage = new Map<string, Map<string, Map<string, "add" | "remove">>>();

  const sorted = [...reactionMsgs].sort((a, b) => a.timestamp - b.timestamp);
  for (const msg of sorted) {
    const meta = msg.metadata ?? {};
    const targetId = meta.message_id;
    const emoji = meta.emoji;
    const action = meta.action as "add" | "remove" | undefined;
    if (!targetId || !emoji || !action) continue;

    if (!byMessage.has(targetId)) byMessage.set(targetId, new Map());
    const byEmoji = byMessage.get(targetId)!;
    if (!byEmoji.has(emoji)) byEmoji.set(emoji, new Map());
    byEmoji.get(emoji)!.set(msg.sender_did, action);
  }

  const result = new Map<string, ReactionSummary[]>();
  for (const [msgId, byEmoji] of byMessage) {
    const summaries: ReactionSummary[] = [];
    for (const [emoji, reactors] of byEmoji) {
      let count = 0;
      let reactedByMe = false;
      for (const [did, action] of reactors) {
        if (action === "add") {
          count++;
          if (did === localDid) reactedByMe = true;
        }
      }
      if (count > 0) summaries.push({ emoji, count, reacted_by_me: reactedByMe });
    }
    if (summaries.length > 0) result.set(msgId, summaries);
  }
  return result;
}

export function GroupView({ groupId }: GroupViewProps) {
  const localDid = useIdentityStore((s) => s.did);
  const setActiveConversation = useMessagingStore((s) => s.setActiveConversation);
  const groupMessageTick = useMessagingStore((s) => s.groupMessageTick);
  const queryClient = useQueryClient();
  const bottomRef = useRef<HTMLDivElement>(null);

  const { data: group } = useQuery({
    queryKey: ["groups"],
    queryFn: groupsApi.list,
    select: (groups) => groups.find((g) => g.id === groupId),
  });

  const { data: members = [] } = useQuery({
    queryKey: ["group-members", groupId],
    queryFn: () => groupsApi.listMembers(groupId),
    staleTime: 30_000,
  });

  const { data: messages = [], refetch } = useQuery({
    queryKey: ["messages", "group", groupId],
    queryFn: () => messagesApi.getGroup(groupId),
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnMount: "always",
  });

  // Refetch when a GroupMessageReceived WS event arrives for this group.
  useEffect(() => {
    if (groupMessageTick > 0) void refetch();
    // refetch is stable; omit from deps intentionally.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groupMessageTick]);

  // On mount, jump to bottom immediately.
  useEffect(() => {
    bottomRef.current?.scrollIntoView();
  }, []);

  // Smooth-scroll to bottom when new messages arrive.
  const prevCountRef = useRef(messages.length);
  useEffect(() => {
    if (messages.length > prevCountRef.current) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    }
    prevCountRef.current = messages.length;
  }, [messages.length]);

  // Split reaction messages from regular messages and aggregate.
  const reactionMessages = messages.filter((m) => m.metadata?.type === "reaction");
  const sortedMessages = messages.filter((m) => m.metadata?.type !== "reaction");
  const reactionsByMsgId = aggregateGroupReactions(reactionMessages, localDid);

  const handleReact = useCallback(
    async (messageId: string, emoji: string) => {
      const myReactions = reactionsByMsgId.get(messageId) ?? [];
      const existing = myReactions.find((r) => r.emoji === emoji);
      try {
        if (existing?.reacted_by_me) {
          await reactionsApi.removeGroup(messageId, emoji, groupId);
        } else {
          await reactionsApi.addGroup(messageId, emoji, groupId);
        }
        void queryClient.invalidateQueries({ queryKey: ["messages", "group", groupId] });
      } catch (e) {
        console.error("Failed to send group reaction:", e);
      }
    },
    [reactionsByMsgId, groupId, queryClient]
  );

  if (!group) return null;

  return (
    <div className="flex h-full flex-col">
      <GroupHeader group={group} onLeave={() => setActiveConversation(null)} />

      <div className="flex flex-1 min-h-0">
        {/* Chat area */}
        <div className="flex flex-1 flex-col min-w-0">
          <ScrollArea className="flex-1 px-4 py-4">
            {sortedMessages.length === 0 ? (
              <div className="flex h-40 items-center justify-center">
                <p className="text-sm text-surface-400">No messages yet. Say something!</p>
              </div>
            ) : (
              <div className="flex flex-col gap-1.5">
                {sortedMessages.map((msg, i) => {
                  const isOwn = msg.sender_did === localDid;
                  const showDivider =
                    i === 0 || isDifferentDay(sortedMessages[i - 1].timestamp, msg.timestamp);
                  const showSender = groupShowSenderAbove(sortedMessages, i, localDid);

                  return (
                    <div key={msg.id}>
                      {showDivider && <DateDivider timestamp={msg.timestamp} />}
                      <GroupMessageBubble
                        message={msg}
                        isOwn={isOwn}
                        showSender={showSender}
                        reactions={reactionsByMsgId.get(msg.id) ?? []}
                        onReact={handleReact}
                      />
                    </div>
                  );
                })}
                <div ref={bottomRef} />
              </div>
            )}
          </ScrollArea>

          <GroupMessageInput groupId={groupId} />
        </div>

        {/* Member sidebar */}
        <div className="w-48 shrink-0 border-l border-surface-200 dark:border-surface-800 overflow-y-auto">
          <div className="px-3 py-3">
            <p className="text-xs font-medium text-surface-500 uppercase tracking-wide mb-2">
              Members ({members.length})
            </p>
            <div className="flex flex-col gap-0.5">
              {members.map((m) => {
                const isMe = m.did === localDid;
                return (
                  <div
                    key={m.did}
                    className="flex items-center gap-2 rounded-md px-2 py-1.5 text-xs text-surface-700 dark:text-surface-300"
                  >
                    <User className="h-3 w-3 shrink-0 text-surface-400" />
                    <span className="truncate">
                      {m.display_name ?? m.did.slice(-12)}
                      {isMe && <span className="ml-1 text-surface-400">(you)</span>}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

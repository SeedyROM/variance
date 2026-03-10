import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Avatar } from "../ui/Avatar";
import { ScrollArea } from "../ui/ScrollArea";
import { StatusDot, StatusLabel } from "../ui/StatusIndicator";
import { MessageBubble } from "./MessageBubble";
import { MessageInput } from "./MessageInput";
import { TypingIndicator } from "./TypingIndicator";
import { DateDivider } from "./DateDivider";
import { messagesApi, reactionsApi, receiptsApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useMessagingStore } from "../../stores/messagingStore";
import { isDifferentDay } from "../../utils/time";
import type { DirectMessage, ReactionSummary } from "../../api/types";

interface MessageViewProps {
  peerDid: string;
}

// Initial and paginated load size. Backend supports cursor pagination via ?before=<ts>.
const PAGE_SIZE = 50;

/** Squash reaction messages into per-message, per-emoji counts. */
function aggregateReactions(
  reactionMsgs: DirectMessage[],
  localDid: string | null
): Map<string, ReactionSummary[]> {
  // For each target message, track the latest action per reactor per emoji.
  const byMessage = new Map<string, Map<string, Map<string, "add" | "remove">>>();

  // Process in chronological order so later actions overwrite earlier ones.
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
      const safeCount = Math.max(0, count);
      if (safeCount > 0) summaries.push({ emoji, count: safeCount, reacted_by_me: reactedByMe });
    }
    if (summaries.length > 0) result.set(msgId, summaries);
  }
  return result;
}

export function MessageView({ peerDid }: MessageViewProps) {
  const localDid = useIdentityStore((s) => s.did);
  const queryClient = useQueryClient();
  const scrollRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const topSentinelRef = useRef<HTMLDivElement>(null);

  // Older pages fetched when scrolling to the top.
  const [olderMessages, setOlderMessages] = useState<DirectMessage[]>([]);
  const [hasMore, setHasMore] = useState(true);
  const [loadingOlder, setLoadingOlder] = useState(false);

  const { data: messages = [] } = useQuery({
    queryKey: ["messages", peerDid],
    queryFn: async () => {
      const msgs = await messagesApi.getDirect(peerDid, undefined, PAGE_SIZE);
      // Fetching messages updates last_read_at on the server — refresh the
      // conversations list so the unread badge clears immediately.
      void queryClient.invalidateQueries({ queryKey: ["conversations"] });
      return msgs;
    },
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnMount: "always",
  });

  const typingUsersSet = useMessagingStore((s) => s.typingUsers.get(peerDid));
  const typingUsers = typingUsersSet ? Array.from(typingUsersSet) : [];

  // Merge older pages with the current page, deduplicate, sort chronologically.
  const allMessages = [...olderMessages, ...messages]
    .filter((msg, i, arr) => arr.findIndex((m) => m.id === msg.id) === i)
    .sort((a, b) => a.timestamp - b.timestamp);

  // Split reaction messages from regular messages.
  const reactionMessages = allMessages.filter((m) => m.metadata?.type === "reaction");
  const sortedMessages = allMessages.filter((m) => m.metadata?.type !== "reaction");
  const reactionsByMsgId = useMemo(
    () => aggregateReactions(reactionMessages, localDid),
    // reactionMessages identity changes when allMessages changes, which is correct
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [reactionMessages, localDid]
  );

  const handleReact = useCallback(
    async (messageId: string, emoji: string) => {
      const myReactions = reactionsByMsgId.get(messageId) ?? [];
      const existing = myReactions.find((r) => r.emoji === emoji);
      try {
        if (existing?.reacted_by_me) {
          await reactionsApi.remove(messageId, emoji, peerDid);
        } else {
          await reactionsApi.add(messageId, emoji, peerDid);
        }
        void queryClient.invalidateQueries({ queryKey: ["messages", peerDid] });
      } catch (e) {
        console.error("Failed to send reaction:", e);
      }
    },
    [reactionsByMsgId, peerDid, queryClient]
  );

  const loadOlder = useCallback(async () => {
    if (loadingOlder || !hasMore) return;

    const oldestTimestamp = sortedMessages[0]?.timestamp;
    if (oldestTimestamp === undefined) return;

    setLoadingOlder(true);

    // Capture scroll height before the DOM update so we can restore position.
    const container = scrollRef.current;
    const prevScrollHeight = container?.scrollHeight ?? 0;

    try {
      const page = await messagesApi.getDirect(peerDid, oldestTimestamp);

      if (page.length === 0) {
        setHasMore(false);
        return;
      }

      if (page.length < PAGE_SIZE) setHasMore(false);

      setOlderMessages((prev) =>
        [...page, ...prev].filter((m, i, arr) => arr.findIndex((x) => x.id === m.id) === i)
      );

      // After React re-renders with the new messages, pin scroll so the user
      // stays at the same visual position instead of jumping to the top.
      requestAnimationFrame(() => {
        if (container) {
          container.scrollTop += container.scrollHeight - prevScrollHeight;
        }
      });
    } finally {
      setLoadingOlder(false);
    }
    // sortedMessages changes every render; use the length + first id as stable deps.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadingOlder, hasMore, sortedMessages[0]?.timestamp, peerDid]);

  // Fire loadOlder when the top sentinel scrolls into the scroll container's viewport.
  useEffect(() => {
    const sentinel = topSentinelRef.current;
    const container = scrollRef.current;
    if (!sentinel || !container) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting) void loadOlder();
      },
      { root: container, threshold: 0 }
    );

    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [loadOlder]);

  // Scroll to bottom when the newest page gains messages (new send/receive), but
  // not when older pages are prepended (that would pull the user away from history).
  const prevNewestCountRef = useRef(messages.length);
  useEffect(() => {
    if (messages.length > prevNewestCountRef.current) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    }
    prevNewestCountRef.current = messages.length;
  }, [messages.length]);

  // On mount (or conversation switch, handled by key=), jump straight to the bottom.
  useEffect(() => {
    bottomRef.current?.scrollIntoView();
  }, []);

  // Send read receipts for incoming messages. Track which IDs we've already
  // receipted in a ref so we don't re-fire on every query refetch.
  const receiptedIds = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (!localDid) return;
    for (const msg of messages) {
      if (msg.sender_did === localDid) continue;
      if (receiptedIds.current.has(msg.id)) continue;
      receiptedIds.current.add(msg.id);
      void receiptsApi.sendRead(msg.id, msg.sender_did).catch(() => {});
    }
  }, [messages, localDid]);

  // Try to get the peer's display name: WS-cached name → message sender_username → truncated DID
  const peerNames = useMessagingStore((s) => s.peerNames);
  const messageUsername = sortedMessages.find(
    (m) => m.sender_did === peerDid && m.sender_username
  )?.sender_username;
  const peerUsername = peerNames.get(peerDid) ?? messageUsername;

  const isSelf = peerDid === localDid;
  const presenceMap = useMessagingStore((s) => s.presenceMap);
  const isOnline = isSelf || (presenceMap.get(peerDid) ?? false);
  const headerName = isSelf ? "Notes to Self" : (peerUsername ?? peerDid.slice(-16));

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-center gap-3 border-b border-surface-200 px-4 py-3 dark:border-surface-800">
        <Avatar did={peerDid} size="md" />
        <div className="cursor-default min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <p className="text-sm font-semibold text-surface-900 dark:text-surface-50 truncate">
              {headerName}
            </p>
            <StatusDot online={isOnline} />
            <StatusLabel online={isOnline} />
          </div>
          <p className="text-xs text-surface-500 font-mono truncate">{peerDid}</p>
        </div>
      </div>

      {/* Messages */}
      <ScrollArea ref={scrollRef} className="flex-1 px-4 py-4">
        {/* Sentinel observed by IntersectionObserver to trigger older-page loads. */}
        <div ref={topSentinelRef} />

        {loadingOlder && (
          <div className="flex justify-center py-2">
            <p className="text-xs text-surface-400">Loading earlier messages…</p>
          </div>
        )}

        {sortedMessages.length === 0 ? (
          <div className="flex h-40 items-center justify-center">
            <p className="text-sm text-surface-400">
              {isSelf ? "No messages yet. Jot something down!" : "No messages yet. Say hello!"}
            </p>
          </div>
        ) : (
          <div className="flex flex-col gap-1.5">
            {sortedMessages.map((msg, i) => {
              const showDivider =
                i === 0 || isDifferentDay(sortedMessages[i - 1].timestamp, msg.timestamp);
              return (
                <div key={msg.id}>
                  {showDivider && <DateDivider timestamp={msg.timestamp} />}
                  <MessageBubble
                    message={msg}
                    isOwn={msg.sender_did === localDid}
                    reactions={reactionsByMsgId.get(msg.id) ?? []}
                    onReact={handleReact}
                    peerDid={peerDid}
                  />
                </div>
              );
            })}
            <div ref={bottomRef} />
          </div>
        )}
      </ScrollArea>

      {/* Typing indicator */}
      <TypingIndicator users={typingUsers} />

      {/* Input */}
      <MessageInput peerDid={peerDid} />
    </div>
  );
}

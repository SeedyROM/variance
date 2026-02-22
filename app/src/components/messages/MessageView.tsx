import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Avatar } from "../ui/Avatar";
import { ScrollArea } from "../ui/ScrollArea";
import { StatusDot, StatusLabel } from "../ui/StatusIndicator";
import { MessageBubble } from "./MessageBubble";
import { MessageInput } from "./MessageInput";
import { TypingIndicator } from "./TypingIndicator";
import { DateDivider } from "./DateDivider";
import { messagesApi, typingApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useMessagingStore } from "../../stores/messagingStore";
import { isDifferentDay } from "../../utils/time";
import type { DirectMessage } from "../../api/types";

interface MessageViewProps {
  peerDid: string;
}

// Must match the backend default limit in get_direct_messages.
const PAGE_SIZE = 1024;

export function MessageView({ peerDid }: MessageViewProps) {
  const localDid = useIdentityStore((s) => s.did);
  const scrollRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const topSentinelRef = useRef<HTMLDivElement>(null);

  // Older pages fetched when scrolling to the top.
  const [olderMessages, setOlderMessages] = useState<DirectMessage[]>([]);
  const [hasMore, setHasMore] = useState(true);
  const [loadingOlder, setLoadingOlder] = useState(false);

  const { data: messages = [], refetch } = useQuery({
    queryKey: ["messages", peerDid],
    queryFn: () => messagesApi.getDirect(peerDid),
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnMount: "always",
  });

  // When a DirectMessageReceived WebSocket event arrives, useWebSocket bumps this
  // tick. We call our own refetch() here — using the peerDid already wired into
  // this query — instead of relying on query-key matching in the WebSocket handler.
  const inboundTick = useMessagingStore((s) => s.inboundMessageTick);
  useEffect(() => {
    if (inboundTick > 0) {
      void refetch();
    }
    // refetch is a stable function reference from React Query; omit from deps intentionally.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inboundTick]);

  const { data: typingData } = useQuery({
    queryKey: ["typing", peerDid],
    queryFn: () => typingApi.get(peerDid),
    refetchInterval: 2000,
  });

  // Merge older pages with the current page, deduplicate, sort chronologically.
  const sortedMessages = [...olderMessages, ...messages]
    .filter((msg, i, arr) => arr.findIndex((m) => m.id === msg.id) === i)
    .sort((a, b) => a.timestamp - b.timestamp);

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

  // Try to get the peer's display name: WS-cached name → message sender_username → truncated DID
  const peerNames = useMessagingStore((s) => s.peerNames);
  const messageUsername = sortedMessages.find(
    (m) => m.sender_did === peerDid && m.sender_username
  )?.sender_username;
  const peerUsername = peerNames.get(peerDid) ?? messageUsername;

  const presenceMap = useMessagingStore((s) => s.presenceMap);
  const isOnline = presenceMap.get(peerDid) ?? false;

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-center gap-3 border-b border-surface-200 px-4 py-3 dark:border-surface-800">
        <Avatar did={peerDid} size="md" />
        <div className="cursor-default min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <p className="text-sm font-semibold text-surface-900 dark:text-surface-50 truncate">
              {peerUsername ?? peerDid.slice(-16)}
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
            <p className="text-sm text-surface-400">No messages yet. Say hello!</p>
          </div>
        ) : (
          <div className="flex flex-col gap-1.5">
            {sortedMessages.map((msg, i) => {
              const showDivider =
                i === 0 || isDifferentDay(sortedMessages[i - 1].timestamp, msg.timestamp);
              return (
                <div key={msg.id}>
                  {showDivider && <DateDivider timestamp={msg.timestamp} />}
                  <MessageBubble message={msg} isOwn={msg.sender_did === localDid} />
                </div>
              );
            })}
            <div ref={bottomRef} />
          </div>
        )}
      </ScrollArea>

      {/* Typing indicator */}
      <TypingIndicator users={typingData?.users ?? []} />

      {/* Input */}
      <MessageInput peerDid={peerDid} />
    </div>
  );
}

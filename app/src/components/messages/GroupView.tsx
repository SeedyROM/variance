import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useToastStore } from "../../stores/toastStore";
import { GroupHeader } from "./GroupHeader";
import { GroupMessageBubble } from "./GroupMessageBubble";
import { TypingIndicator } from "./TypingIndicator";
import { DateDivider } from "./DateDivider";
import { ScrollArea } from "../ui/ScrollArea";
import { Avatar } from "../ui/Avatar";
import { StatusDot } from "../ui/StatusIndicator";
import { MessageEditor } from "./MessageEditor";
import { messagesApi, groupsApi, reactionsApi, groupReceiptsApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useMessagingStore } from "../../stores/messagingStore";
import { isDifferentDay } from "../../utils/time";
import { cn } from "../../utils/cn";
import { Snowflake, MessageSquare, Copy } from "lucide-react";
import { ContextMenu, type ContextMenuItem } from "../ui/ContextMenu";
import type { GroupMessage, GroupMemberInfo, ReactionSummary } from "../../api/types";

export type BubblePosition = "solo" | "first" | "middle" | "last";

// Initial and paginated load size. Backend supports cursor pagination via ?before=<ts>.
const PAGE_SIZE = 50;

/** Returns true when the viewport is narrower than the given pixel width. */
function useMediaQuery(maxWidth: number): boolean {
  const [matches, setMatches] = useState(
    () => typeof window !== "undefined" && window.innerWidth < maxWidth
  );
  useEffect(() => {
    const mql = window.matchMedia(`(max-width: ${maxWidth - 1}px)`);
    const handler = (e: MediaQueryListEvent) => setMatches(e.matches);
    setMatches(mql.matches);
    mql.addEventListener("change", handler);
    return () => mql.removeEventListener("change", handler);
  }, [maxWidth]);
  return matches;
}

interface GroupViewProps {
  groupId: string;
  /** Width of the left conversation sidebar (px), used to decide when
   *  the member sidebar should switch to overlay mode. */
  sidebarWidth?: number;
}

function GroupMessageInput({ groupId }: { groupId: string }) {
  const queryClient = useQueryClient();
  const addToast = useToastStore((s) => s.addToast);

  const sendMutation = useMutation({
    mutationFn: (message: string) => groupsApi.sendMessage(groupId, message),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["messages", "group", groupId] });
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
    },
    onError: (e) => addToast(String(e), "error"),
  });

  return (
    <MessageEditor
      placeholder="Message group"
      onSend={(md) => sendMutation.mutate(md)}
      isPending={sendMutation.isPending}
      typing={{ recipient: groupId, isGroup: true, cooldownMs: 500, stopDelayMs: 1_500 }}
    />
  );
}

/** Is this message in the same consecutive sender run as its neighbour? */
function isSameRun(messages: GroupMessage[], i: number, j: number): boolean {
  if (j < 0 || j >= messages.length) return false;
  return (
    messages[i].sender_did === messages[j].sender_did &&
    !isDifferentDay(messages[i].timestamp, messages[j].timestamp)
  );
}

function bubblePosition(messages: GroupMessage[], index: number): BubblePosition {
  const prevSame = isSameRun(messages, index, index - 1);
  const nextSame = isSameRun(messages, index, index + 1);
  if (prevSame && nextSame) return "middle";
  if (prevSame) return "last";
  if (nextSame) return "first";
  return "solo";
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

export function GroupView({ groupId, sidebarWidth = 288 }: GroupViewProps) {
  const localDid = useIdentityStore((s) => s.did);
  const presenceMap = useMessagingStore((s) => s.presenceMap);
  const setActiveConversation = useMessagingStore((s) => s.setActiveConversation);
  const typingUsersSet = useMessagingStore((s) => s.typingUsers.get(`group:${groupId}`));
  const typingUsers = typingUsersSet ? Array.from(typingUsersSet) : [];
  const queryClient = useQueryClient();
  const scrollRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const topSentinelRef = useRef<HTMLDivElement>(null);

  // Older pages fetched when scrolling to the top.
  const [olderMessages, setOlderMessages] = useState<GroupMessage[]>([]);
  const [hasMore, setHasMore] = useState(true);
  const [loadingOlder, setLoadingOlder] = useState(false);

  // Sidebar toggle — hidden by default on narrow windows.
  // Use a responsive check: if the main content area (window - left sidebar - resize handle)
  // is < 500px, treat as narrow so the member sidebar overlays instead of eating chat space.
  const isNarrow = useMediaQuery(sidebarWidth + 4 + 500); // 4px for resize handle
  const [sidebarOpen, setSidebarOpen] = useState(!isNarrow);
  // Auto-close sidebar when the window shrinks below the breakpoint
  useEffect(() => {
    if (isNarrow) setSidebarOpen(false);
  }, [isNarrow]);

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

  const { data: messages = [] } = useQuery({
    queryKey: ["messages", "group", groupId],
    queryFn: async () => {
      const msgs = await messagesApi.getGroup(groupId, undefined, PAGE_SIZE);
      // Fetching messages updates last_read_at on the server — refresh the
      // groups list so the unread badge clears immediately.
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      return msgs;
    },
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnMount: "always",
  });

  // Merge older pages with the current page, deduplicate, sort chronologically.
  const allMessages = useMemo(
    () =>
      [...olderMessages, ...messages]
        .filter((msg, i, arr) => arr.findIndex((m) => m.id === msg.id) === i)
        .sort((a, b) => a.timestamp - b.timestamp),
    [olderMessages, messages]
  );

  // On mount, jump to bottom immediately.
  useEffect(() => {
    bottomRef.current?.scrollIntoView();
  }, []);

  // Smooth-scroll to bottom when new messages arrive (newest page grows),
  // but not when older pages are prepended.
  const prevNewestCountRef = useRef(messages.length);
  useEffect(() => {
    if (messages.length > prevNewestCountRef.current) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    }
    prevNewestCountRef.current = messages.length;
  }, [messages.length]);

  const loadOlder = useCallback(async () => {
    if (loadingOlder || !hasMore) return;

    // Find the oldest non-reaction, non-role-change message timestamp for the cursor.
    const oldestTimestamp = allMessages.find(
      (m) => m.metadata?.type !== "reaction" && m.metadata?.type !== "role_change"
    )?.timestamp;
    if (oldestTimestamp === undefined) return;

    setLoadingOlder(true);

    const container = scrollRef.current;
    const prevScrollHeight = container?.scrollHeight ?? 0;

    try {
      const page = await messagesApi.getGroup(groupId, oldestTimestamp);

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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadingOlder, hasMore, allMessages[0]?.timestamp, groupId]);

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

  // Send read receipts for incoming group messages from other members.
  // Track which IDs we've already receipted to avoid re-firing on refetch.
  const receiptedIds = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (!localDid) return;
    const unread = messages.filter(
      (m) =>
        m.sender_did !== localDid &&
        m.metadata?.type !== "reaction" &&
        m.metadata?.type !== "role_change" &&
        !receiptedIds.current.has(m.id)
    );
    if (unread.length === 0) return;
    for (const m of unread) receiptedIds.current.add(m.id);
    void groupReceiptsApi
      .sendRead(
        groupId,
        unread.map((m) => m.id)
      )
      .catch(() => {});
  }, [messages, localDid, groupId]);

  // Split reaction messages from regular messages and aggregate.
  const reactionMessages = allMessages.filter((m) => m.metadata?.type === "reaction");
  const sortedMessages = allMessages.filter(
    (m) => m.metadata?.type !== "reaction" && m.metadata?.type !== "role_change"
  );
  const reactionsByMsgId = useMemo(
    () => aggregateGroupReactions(reactionMessages, localDid),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [reactionMessages, localDid]
  );

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
      <GroupHeader
        group={group}
        onLeave={() => setActiveConversation(null)}
        onToggleMembers={() => setSidebarOpen((v) => !v)}
        membersOpen={sidebarOpen}
      />

      <div className="relative flex flex-1 min-h-0">
        {/* Chat area */}
        <div className="flex flex-1 flex-col min-w-0">
          <ScrollArea ref={scrollRef} className="flex-1 px-4 py-4">
            {/* Sentinel observed by IntersectionObserver to trigger older-page loads. */}
            <div ref={topSentinelRef} />

            {loadingOlder && (
              <div className="flex justify-center py-2">
                <p className="text-xs text-surface-400">Loading earlier messages…</p>
              </div>
            )}

            {sortedMessages.length === 0 ? (
              <div className="flex items-center justify-center px-8 py-8">
                <p className="text-sm text-surface-400">
                  {group.your_role !== "admin" && members.length > 1
                    ? "You joined this group. Messages sent before you joined are not available."
                    : "No messages yet. Say something!"}
                </p>
              </div>
            ) : (
              <div className="flex flex-col">
                {sortedMessages.map((msg, i) => {
                  const isOwn = msg.sender_did === localDid;
                  const pos = bubblePosition(sortedMessages, i);
                  const showDivider =
                    i === 0 || isDifferentDay(sortedMessages[i - 1].timestamp, msg.timestamp);
                  const isOnline =
                    msg.sender_did === localDid || (presenceMap.get(msg.sender_did) ?? false);

                  // Spacing: tight (2px) within a run, normal (6px) between runs
                  const isRunStart = pos === "solo" || pos === "first";
                  const mt = i === 0 ? "" : isRunStart ? "mt-1.5" : "mt-0.5";

                  return (
                    <div key={msg.id} className={mt}>
                      {showDivider && <DateDivider timestamp={msg.timestamp} />}
                      <GroupMessageBubble
                        message={msg}
                        isOwn={isOwn}
                        position={pos}
                        senderOnline={isOnline}
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

          <TypingIndicator users={typingUsers} />
          {group.is_frozen ? (
            <div className="flex items-center justify-center gap-2 border-t border-surface-200 dark:border-surface-800 px-4 py-3 bg-surface-100 dark:bg-surface-900/60">
              <Snowflake className="h-4 w-4 text-sky-500" />
              <span className="text-sm text-surface-500">
                This group is frozen. The admin left without transferring ownership.
              </span>
            </div>
          ) : (
            <GroupMessageInput groupId={groupId} />
          )}
        </div>

        {/* Member sidebar — overlays on narrow screens, inline on wide */}
        {sidebarOpen && (
          <>
            {/* Backdrop for narrow overlay */}
            {isNarrow && (
              <div
                className="absolute inset-0 z-10 bg-black/20"
                onClick={() => setSidebarOpen(false)}
              />
            )}
            <MemberSidebar members={members} localDid={localDid} overlay={isNarrow} />
          </>
        )}
      </div>
    </div>
  );
}

function MemberSidebar({
  members,
  localDid,
  overlay,
}: {
  members: GroupMemberInfo[];
  localDid: string | null;
  overlay?: boolean;
}) {
  const presenceMap = useMessagingStore((s) => s.presenceMap);

  const { online, offline } = useMemo(() => {
    const on: GroupMemberInfo[] = [];
    const off: GroupMemberInfo[] = [];
    for (const m of members) {
      const isOnline = m.did === localDid || (presenceMap.get(m.did) ?? false);
      (isOnline ? on : off).push(m);
    }
    const byName = (a: GroupMemberInfo, b: GroupMemberInfo) => {
      const nameA = a.display_name ?? a.did.slice(-12);
      const nameB = b.display_name ?? b.did.slice(-12);
      return nameA.localeCompare(nameB);
    };
    on.sort(byName);
    off.sort(byName);
    return { online: on, offline: off };
  }, [members, presenceMap, localDid]);

  return (
    <div
      className={cn(
        "w-56 shrink-0 border-l border-surface-200 dark:border-surface-800 bg-surface-50 dark:bg-surface-900/50",
        overlay && "absolute right-0 top-0 bottom-0 z-20 shadow-xl"
      )}
    >
      <ScrollArea className="h-full">
        <div className="px-3 py-4 flex flex-col gap-4">
          {/* Online section */}
          <MemberSection
            label={`Online — ${online.length}`}
            members={online}
            online
            localDid={localDid}
          />

          {/* Offline section */}
          {offline.length > 0 && (
            <MemberSection
              label={`Offline — ${offline.length}`}
              members={offline}
              online={false}
              localDid={localDid}
            />
          )}
        </div>
      </ScrollArea>
    </div>
  );
}

function MemberSection({
  label,
  members,
  online,
  localDid,
}: {
  label: string;
  members: GroupMemberInfo[];
  online: boolean;
  localDid: string | null;
}) {
  const setActiveConversation = useMessagingStore((s) => s.setActiveConversation);

  const buildContextItems = useCallback(
    (m: GroupMemberInfo): ContextMenuItem[] => {
      const isMe = m.did === localDid;
      const items: ContextMenuItem[] = [];

      if (!isMe) {
        items.push({
          label: "Send Message",
          icon: <MessageSquare size={14} />,
          onClick: () => setActiveConversation({ type: "dm", peerId: m.did }),
        });
      }

      items.push({
        label: "Copy DID",
        icon: <Copy size={14} />,
        divider: items.length > 0,
        onClick: () => navigator.clipboard.writeText(m.did),
      });

      return items;
    },
    [localDid, setActiveConversation]
  );

  return (
    <div>
      <p className="text-[11px] font-semibold text-surface-500 dark:text-surface-400 uppercase tracking-wider px-1 mb-1.5">
        {label}
      </p>
      <div className="flex flex-col gap-0.5">
        {members.map((m) => {
          const isMe = m.did === localDid;
          const displayName = m.display_name ?? m.did.slice(-12);

          return (
            <ContextMenu key={m.did} items={buildContextItems(m)}>
              <div className="group flex items-center gap-2.5 rounded-md px-2 py-1.5 hover:bg-surface-200/60 dark:hover:bg-surface-800/60 transition-colors">
                {/* Avatar with status dot overlay */}
                <div className="relative shrink-0">
                  <Avatar
                    did={m.did}
                    name={m.display_name ?? undefined}
                    size="sm"
                    className={online ? "" : "opacity-40"}
                  />
                  <StatusDot
                    online={online}
                    size="md"
                    className="absolute -bottom-0.5 -right-0.5 border-2 border-surface-50 dark:border-surface-900"
                  />
                </div>

                {/* Name */}
                <span
                  className={`truncate text-[13px] font-medium ${
                    online
                      ? "text-surface-800 dark:text-surface-200"
                      : "text-surface-400 dark:text-surface-500"
                  }`}
                >
                  {displayName}
                  {isMe && (
                    <span className="ml-1 text-[11px] font-normal text-surface-400 dark:text-surface-500">
                      (you)
                    </span>
                  )}
                </span>
              </div>
            </ContextMenu>
          );
        })}
      </div>
    </div>
  );
}

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
import { messagesApi, groupsApi, reactionsApi, typingApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useMessagingStore } from "../../stores/messagingStore";
import { isDifferentDay } from "../../utils/time";
import { cn } from "../../utils/cn";
import { MessageComposerShell, MAX_MESSAGE_LENGTH } from "./MessageComposerShell";
import type { GroupMessage, GroupMemberInfo, ReactionSummary } from "../../api/types";

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
}

/** Don't send another /typing/start within this window (ms). */
const TYPING_SEND_COOLDOWN_MS = 500;

function GroupMessageInput({ groupId }: { groupId: string }) {
  const [text, setText] = useState("");
  const queryClient = useQueryClient();
  const typingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastTypingSentRef = useRef<number>(0);
  const addToast = useToastStore((s) => s.addToast);

  // Cancel any pending stop-typing timer on unmount to avoid firing stale events
  // for the old group after a conversation switch.
  useEffect(
    () => () => {
      if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
    },
    []
  );

  const sendMutation = useMutation({
    mutationFn: () => groupsApi.sendMessage(groupId, text.trim()),
    onSuccess: () => {
      setText("");
      void queryClient.invalidateQueries({ queryKey: ["messages", "group", groupId] });
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
    },
    onError: (e) => addToast(String(e), "error"),
  });

  const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value;
    setText(value);

    if (!value.trim()) return;
    const now = Date.now();
    if (now - lastTypingSentRef.current >= TYPING_SEND_COOLDOWN_MS) {
      lastTypingSentRef.current = now;
      void typingApi.start({ recipient: groupId, is_group: true });
    }
    if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
    typingTimerRef.current = setTimeout(() => {
      lastTypingSentRef.current = 0;
      void typingApi.stop({ recipient: groupId, is_group: true });
    }, 1500);
  };

  const handleSend = () => {
    if (!text.trim() || text.length > MAX_MESSAGE_LENGTH || sendMutation.isPending) return;
    if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
    lastTypingSentRef.current = 0;
    void typingApi.stop({ recipient: groupId, is_group: true });
    sendMutation.mutate();
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <MessageComposerShell
      charCount={text.length}
      isEmpty={!text.trim()}
      isPending={sendMutation.isPending}
      onSend={handleSend}
    >
      <input
        type="text"
        value={text}
        onChange={handleChange}
        onKeyDown={handleKeyDown}
        placeholder="Message group"
        className="flex-1 min-w-0 text-sm text-surface-900 dark:text-surface-50 bg-transparent focus:outline-none"
      />
    </MessageComposerShell>
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
  const presenceMap = useMessagingStore((s) => s.presenceMap);
  const setActiveConversation = useMessagingStore((s) => s.setActiveConversation);
  const typingUsersSet = useMessagingStore((s) => s.typingUsers.get(`group:${groupId}`));
  const typingUsers = typingUsersSet ? Array.from(typingUsersSet) : [];
  const queryClient = useQueryClient();
  const bottomRef = useRef<HTMLDivElement>(null);

  // Sidebar toggle — hidden by default on narrow windows (<768px)
  const isNarrow = useMediaQuery(768);
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
      const msgs = await messagesApi.getGroup(groupId);
      // Fetching messages updates last_read_at on the server — refresh the
      // groups list so the unread badge clears immediately.
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      return msgs;
    },
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnMount: "always",
  });

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
      <GroupHeader
        group={group}
        onLeave={() => setActiveConversation(null)}
        onToggleMembers={() => setSidebarOpen((v) => !v)}
        membersOpen={sidebarOpen}
      />

      <div className="relative flex flex-1 min-h-0">
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
                  const isOnline =
                    msg.sender_did === localDid || (presenceMap.get(msg.sender_did) ?? false);

                  return (
                    <div key={msg.id}>
                      {showDivider && <DateDivider timestamp={msg.timestamp} />}
                      <GroupMessageBubble
                        message={msg}
                        isOwn={isOwn}
                        showSender={showSender}
                        showAvatar={
                          showSender ||
                          (isOwn &&
                            (i === 0 || sortedMessages[i - 1].sender_did !== msg.sender_did))
                        }
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
          <GroupMessageInput groupId={groupId} />
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
            <div
              key={m.did}
              className="group flex items-center gap-2.5 rounded-md px-2 py-1.5 hover:bg-surface-200/60 dark:hover:bg-surface-800/60 transition-colors"
            >
              {/* Avatar with status dot overlay */}
              <div className="relative shrink-0">
                <Avatar did={m.did} size="sm" className={online ? "" : "opacity-40"} />
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
          );
        })}
      </div>
    </div>
  );
}

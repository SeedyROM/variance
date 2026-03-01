import { useEffect, useRef, useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Send } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import { GroupHeader } from "./GroupHeader";
import { DateDivider } from "./DateDivider";
import { ScrollArea } from "../ui/ScrollArea";
import { messagesApi, groupsApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useMessagingStore } from "../../stores/messagingStore";
import { cn } from "../../utils/cn";
import { isDifferentDay, shortTime } from "../../utils/time";
import type { GroupMessage } from "../../api/types";

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

export function GroupView({ groupId }: GroupViewProps) {
  const localDid = useIdentityStore((s) => s.did);
  const setActiveConversation = useMessagingStore((s) => s.setActiveConversation);
  const groupMessageTick = useMessagingStore((s) => s.groupMessageTick);
  const bottomRef = useRef<HTMLDivElement>(null);

  const { data: group } = useQuery({
    queryKey: ["groups"],
    queryFn: groupsApi.list,
    select: (groups) => groups.find((g) => g.id === groupId),
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

  if (!group) return null;

  return (
    <div className="flex h-full flex-col">
      <GroupHeader group={group} onLeave={() => setActiveConversation(null)} />

      <ScrollArea className="flex-1 px-4 py-4">
        {messages.length === 0 ? (
          <div className="flex h-40 items-center justify-center">
            <p className="text-sm text-surface-400">No messages yet. Say something!</p>
          </div>
        ) : (
          <div className="flex flex-col gap-1.5">
            {messages.map((msg, i) => {
              const isOwn = msg.sender_did === localDid;
              const showDivider =
                i === 0 || isDifferentDay(messages[i - 1].timestamp, msg.timestamp);
              const showSender = groupShowSenderAbove(messages, i, localDid);

              return (
                <div key={msg.id}>
                  {showDivider && <DateDivider timestamp={msg.timestamp} />}

                  <div className={cn("flex flex-col gap-0.5", isOwn ? "items-end" : "items-start")}>
                    {showSender && (
                      <p className="px-1 text-xs font-medium text-surface-500">
                        {msg.sender_username ?? msg.sender_did.slice(-12)}
                      </p>
                    )}

                    <div
                      className={cn(
                        "flex items-end gap-2",
                        isOwn ? "flex-row-reverse" : "flex-row"
                      )}
                    >
                      <div
                        className={cn(
                          "max-w-sm rounded-2xl px-3.5 py-2.5 text-sm cursor-default",
                          isOwn
                            ? "rounded-br-sm bg-primary-500 text-white"
                            : "rounded-bl-sm bg-surface-200 text-surface-900 dark:bg-surface-800 dark:text-surface-50"
                        )}
                      >
                        <div
                          className={cn(
                            "prose prose-sm max-w-none wrap-break-word select-text",
                            isOwn ? "prose-invert" : "dark:prose-invert"
                          )}
                        >
                          <ReactMarkdown remarkPlugins={[remarkBreaks]}>{msg.text}</ReactMarkdown>
                        </div>
                      </div>
                      <span className="text-[10px] text-surface-400 shrink-0">
                        {shortTime(msg.timestamp)}
                      </span>
                    </div>
                  </div>
                </div>
              );
            })}
            <div ref={bottomRef} />
          </div>
        )}
      </ScrollArea>

      <GroupMessageInput groupId={groupId} />
    </div>
  );
}

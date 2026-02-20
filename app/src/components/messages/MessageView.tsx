import { useEffect, useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import { Avatar } from "../ui/Avatar";
import { ScrollArea } from "../ui/ScrollArea";
import { MessageBubble } from "./MessageBubble";
import { MessageInput } from "./MessageInput";
import { TypingIndicator } from "./TypingIndicator";
import { DateDivider } from "./DateDivider";
import { messagesApi, typingApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { isDifferentDay } from "../../utils/time";

interface MessageViewProps {
  peerDid: string;
}

export function MessageView({ peerDid }: MessageViewProps) {
  const localDid = useIdentityStore((s) => s.did);
  const bottomRef = useRef<HTMLDivElement>(null);

  const { data: messages = [] } = useQuery({
    queryKey: ["messages", peerDid],
    queryFn: () => messagesApi.getDirect(peerDid),
    staleTime: 0, // Always consider stale
    refetchOnMount: true,
    refetchOnWindowFocus: true,
    // No polling - rely on WebSocket events for real-time updates
  });

  const { data: typingData } = useQuery({
    queryKey: ["typing", peerDid],
    queryFn: () => typingApi.get(peerDid),
    refetchInterval: 2000,
  });

  // Sort messages by timestamp (oldest first)
  const sortedMessages = [...messages].sort((a, b) => a.timestamp - b.timestamp);

  // Scroll to bottom when new messages arrive
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.length]);

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-center gap-3 border-b border-surface-200 px-4 py-3 dark:border-surface-800">
        <Avatar did={peerDid} size="md" />
        <div className="cursor-default">
          <p className="text-sm font-semibold text-surface-900 dark:text-surface-50">
            {peerDid.slice(-16)}
          </p>
          <p className="text-xs text-surface-500 font-mono">{peerDid}</p>
        </div>
      </div>

      {/* Messages */}
      <ScrollArea className="flex-1 px-4 py-4">
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

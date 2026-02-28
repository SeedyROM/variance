import { useState, useRef, useEffect } from "react";
import { CheckCheck, Clock } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";

import type { DirectMessage, ReactionSummary } from "../../api/types";
import { cn } from "../../utils/cn";
import { shortTime } from "../../utils/time";

import { EmojiBar } from "./EmojiBar";

interface MessageBubbleProps {
  message: DirectMessage;
  isOwn: boolean;
  reactions: ReactionSummary[];
  onReact: (messageId: string, emoji: string) => void;
  peerDid: string;
}

const LONG_PRESS_MS = 500;

export function MessageBubble({ message, isOwn, reactions, onReact }: MessageBubbleProps) {
  const [showTimestamp, setShowTimestamp] = useState(false);
  const [isHovering, setIsHovering] = useState(false);
  const [showEmojiBar, setShowEmojiBar] = useState(false);
  const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const emojiBarHideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Timestamp fades in after 300ms hover
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    if (isHovering) {
      timer = setTimeout(() => setShowTimestamp(true), 300);
    } else {
      setShowTimestamp(false);
    }
    return () => clearTimeout(timer);
  }, [isHovering]);

  const startLongPress = () => {
    longPressTimer.current = setTimeout(() => setShowEmojiBar(true), LONG_PRESS_MS);
  };

  const cancelLongPress = () => {
    if (longPressTimer.current) {
      clearTimeout(longPressTimer.current);
      longPressTimer.current = null;
    }
  };

  // Schedule hiding the emoji bar after a short delay so the mouse has time to
  // travel from the bubble into the absolutely-positioned bar without it vanishing.
  const scheduleHideEmojiBar = () => {
    emojiBarHideTimer.current = setTimeout(() => setShowEmojiBar(false), 150);
  };

  const cancelHideEmojiBar = () => {
    if (emojiBarHideTimer.current) {
      clearTimeout(emojiBarHideTimer.current);
      emojiBarHideTimer.current = null;
    }
  };

  const handleLeave = () => {
    setIsHovering(false);
    scheduleHideEmojiBar();
    cancelLongPress();
  };

  const visibleReactions = reactions.filter((r) => r.count > 0);

  return (
    <div
      className={cn("flex flex-col gap-1", isOwn ? "items-end" : "items-start")}
      onMouseEnter={() => setIsHovering(true)}
      onMouseLeave={handleLeave}
    >
      <div className={cn("flex items-center gap-2", isOwn ? "justify-end" : "justify-start")}>
        {/* Timestamp left of sent messages */}
        {isOwn && (
          <span
            className={cn(
              "text-[10px] text-surface-400 transition-opacity duration-200",
              showTimestamp ? "opacity-100" : "opacity-0"
            )}
          >
            {shortTime(message.timestamp)}
          </span>
        )}

        <div className="relative">
          {/* Emoji bar — shown on long press, floats above the bubble */}
          {showEmojiBar && (
            <div
              className={cn("absolute bottom-full mb-1 z-10", isOwn ? "right-0" : "left-0")}
              // Cancel the pending hide so the bar survives the mouse travelling up to it
              onMouseEnter={() => {
                setIsHovering(true);
                cancelHideEmojiBar();
              }}
              onMouseLeave={handleLeave}
            >
              <EmojiBar
                messageId={message.id}
                reactions={reactions}
                onReact={(emoji) => {
                  setShowEmojiBar(false);
                  onReact(message.id, emoji);
                }}
              />
            </div>
          )}

          <div
            className={cn(
              "relative max-w-sm rounded-2xl px-3.5 py-2.5 text-sm cursor-default select-none",
              isOwn
                ? "rounded-br-sm bg-primary-500 text-white"
                : "rounded-bl-sm bg-surface-200 text-surface-900 dark:bg-surface-800 dark:text-surface-50"
            )}
            onMouseDown={startLongPress}
            onMouseUp={cancelLongPress}
          >
            <div
              className={cn(
                "prose prose-sm max-w-none wrap-break-word select-text",
                isOwn ? "prose-invert" : "dark:prose-invert"
              )}
            >
              <ReactMarkdown remarkPlugins={[remarkBreaks]}>{message.text}</ReactMarkdown>
            </div>
            {isOwn && message.status && (
              <div className="mt-0.5 flex items-center justify-end gap-1">
                {message.status === "pending" && <Clock className="h-3 w-3 text-white/60" />}
                {message.status === "sent" && <CheckCheck className="h-3 w-3 text-white/70" />}
                {message.status === "failed" && (
                  <span className="text-[10px] text-white/60">Failed</span>
                )}
              </div>
            )}
          </div>
        </div>

        {/* Timestamp right of received messages */}
        {!isOwn && (
          <span
            className={cn(
              "text-[10px] text-surface-400 transition-opacity duration-200",
              showTimestamp ? "opacity-100" : "opacity-0"
            )}
          >
            {shortTime(message.timestamp)}
          </span>
        )}
      </div>

      {/* Reaction pills */}
      {visibleReactions.length > 0 && (
        <div className={cn("flex flex-wrap gap-1 mb-0.5", isOwn ? "justify-end" : "justify-start")}>
          {visibleReactions.map((r) => (
            <button
              key={r.emoji}
              onClick={() => onReact(message.id, r.emoji)}
              className={cn(
                "flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs transition-colors",
                r.reacted_by_me
                  ? "border-primary-400 bg-primary-100 text-primary-700 dark:border-primary-600 dark:bg-primary-900/40 dark:text-primary-300"
                  : "border-surface-300 bg-surface-100 text-surface-700 hover:border-surface-400 dark:border-surface-700 dark:bg-surface-800 dark:text-surface-300"
              )}
            >
              <span>{r.emoji}</span>
              <span>{r.count}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

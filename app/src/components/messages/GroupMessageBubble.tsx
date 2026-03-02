import { useState, useRef, useEffect } from "react";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";

import type { GroupMessage, ReactionSummary } from "../../api/types";
import { cn } from "../../utils/cn";
import { shortTime } from "../../utils/time";
import { Avatar } from "../ui/Avatar";
import { StatusDot } from "../ui/StatusIndicator";

import { EmojiBar } from "./EmojiBar";

/** Width of avatar column (avatar sm = 28px). Used for alignment spacer. */
const AVATAR_COL_W = "w-7";

interface GroupMessageBubbleProps {
  message: GroupMessage;
  isOwn: boolean;
  showSender: boolean;
  /** Show the avatar for this message (first in a consecutive run). */
  showAvatar?: boolean;
  /** Whether the message sender is currently online. */
  senderOnline?: boolean;
  reactions: ReactionSummary[];
  onReact: (messageId: string, emoji: string) => void;
}

const LONG_PRESS_MS = 500;

export function GroupMessageBubble({
  message,
  isOwn,
  showSender,
  showAvatar = false,
  senderOnline = false,
  reactions,
  onReact,
}: GroupMessageBubbleProps) {
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

  const avatarElement = showAvatar ? (
    <div className="relative shrink-0 self-end">
      <Avatar did={message.sender_did} size="sm" />
      <StatusDot
        online={senderOnline}
        size="md"
        className="absolute -bottom-0.5 -right-0.5 border-2 border-white dark:border-surface-950"
      />
    </div>
  ) : (
    /* Spacer to keep alignment when avatar is hidden (continuation messages) */
    <div className={cn(AVATAR_COL_W, "shrink-0")} />
  );

  return (
    <div
      className={cn("flex flex-col gap-0.5", isOwn ? "items-end" : "items-start")}
      onMouseEnter={() => setIsHovering(true)}
      onMouseLeave={handleLeave}
    >
      {showSender && (
        <p className={cn("px-1 text-xs font-medium text-surface-500", !isOwn && "ml-9")}>
          {message.sender_username ?? message.sender_did.slice(-12)}
        </p>
      )}

      <div className={cn("flex items-end gap-2", isOwn ? "flex-row-reverse" : "flex-row")}>
        {/* Avatar (or alignment spacer) */}
        {avatarElement}

        {/* Bubble + reactions stacked together so reactions align under the bubble */}
        <div className={cn("flex flex-col gap-0.5", isOwn ? "items-end" : "items-start")}>
          <div className="relative">
            {/* Emoji bar — shown on long press, floats above the bubble */}
            {showEmojiBar && (
              <div
                className={cn("absolute bottom-full mb-1 z-10", isOwn ? "right-0" : "left-0")}
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
            </div>
          </div>

          {/* Reaction pills — directly under the bubble */}
          {visibleReactions.length > 0 && (
            <div className="flex flex-wrap gap-1">
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

        {/* Timestamp on the outer side of the bubble */}
        <span
          className={cn(
            "self-center text-[10px] text-surface-400 transition-opacity duration-200",
            showTimestamp ? "opacity-100" : "opacity-0"
          )}
        >
          {shortTime(message.timestamp)}
        </span>
      </div>
    </div>
  );
}

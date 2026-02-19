import { useState, useEffect } from "react";
import { cn } from "../../utils/cn";
import { shortTime } from "../../utils/time";
import type { DirectMessage } from "../../api/types";

interface MessageBubbleProps {
  message: DirectMessage;
  isOwn: boolean;
}

export function MessageBubble({ message, isOwn }: MessageBubbleProps) {
  const [showTimestamp, setShowTimestamp] = useState(false);
  const [isHovering, setIsHovering] = useState(false);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;

    if (isHovering) {
      // Show timestamp after 500ms
      timer = setTimeout(() => setShowTimestamp(true), 500);
    } else {
      // Hide instantly
      setShowTimestamp(false);
    }

    return () => clearTimeout(timer);
  }, [isHovering]);

  return (
    <div
      className={cn("flex items-center gap-2", isOwn ? "justify-end" : "justify-start")}
      onMouseEnter={() => setIsHovering(true)}
      onMouseLeave={() => setIsHovering(false)}
    >
      {/* Timestamp on left for sent messages */}
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

      <div
        className={cn(
          "relative max-w-sm rounded-2xl px-3.5 py-2.5 text-sm cursor-default",
          isOwn
            ? "rounded-br-sm bg-primary-500 text-white"
            : "rounded-bl-sm bg-surface-200 text-surface-900 dark:bg-surface-800 dark:text-surface-50"
        )}
      >
        <p className="whitespace-pre-wrap break-words select-text">{message.text}</p>
      </div>

      {/* Timestamp on right for received messages */}
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
  );
}

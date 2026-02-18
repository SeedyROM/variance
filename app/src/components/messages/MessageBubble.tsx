import { cn } from "../../utils/cn";
import { shortTime } from "../../utils/time";
import type { DirectMessage } from "../../api/types";

interface MessageBubbleProps {
  message: DirectMessage;
  isOwn: boolean;
}

export function MessageBubble({ message, isOwn }: MessageBubbleProps) {
  return (
    <div className={cn("group flex", isOwn ? "justify-end" : "justify-start")}>
      <div
        className={cn(
          "relative max-w-sm rounded-2xl px-3.5 py-2.5 text-sm",
          isOwn
            ? "rounded-br-sm bg-primary-500 text-white"
            : "rounded-bl-sm bg-surface-200 text-surface-900 dark:bg-surface-800 dark:text-surface-50"
        )}
      >
        <p className="whitespace-pre-wrap break-words">{message.text}</p>
        <p
          className={cn(
            "mt-1 text-right text-[10px] opacity-0 transition-opacity group-hover:opacity-100",
            isOwn ? "text-white/70" : "text-surface-400"
          )}
        >
          {shortTime(message.timestamp)}
        </p>
      </div>
    </div>
  );
}

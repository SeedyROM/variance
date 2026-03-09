import { Send } from "lucide-react";
import { cn } from "../../utils/cn";

export const MAX_MESSAGE_LENGTH = 2048;

interface MessageComposerShellProps {
  charCount: number;
  isEmpty: boolean;
  isPending: boolean;
  onSend: () => void;
  children: React.ReactNode;
}

/**
 * Shared shell for DM and group message inputs.
 *
 * Renders the border container (turns red when over limit), the countdown
 * counter (appears in the last 200 chars), and the send button. The actual
 * input element (TipTap editor or plain <input>) is passed as children.
 */
export function MessageComposerShell({
  charCount,
  isEmpty,
  isPending,
  onSend,
  children,
}: MessageComposerShellProps) {
  const isOverLimit = charCount > MAX_MESSAGE_LENGTH;
  const showCounter = charCount > MAX_MESSAGE_LENGTH - 200;

  return (
    <div className="border-t border-surface-200 bg-surface-50 px-4 py-3 dark:border-surface-800 dark:bg-surface-900">
      <div
        className={cn(
          "flex items-center gap-2 rounded-xl border bg-white px-3 py-2 focus-within:ring-2 dark:bg-surface-950",
          isOverLimit
            ? "border-red-500 focus-within:border-red-500 focus-within:ring-red-500/20"
            : "border-surface-300 focus-within:border-primary-500 focus-within:ring-primary-500/20 dark:border-surface-700"
        )}
      >
        {children}
        {showCounter && (
          <span
            className={cn(
              "shrink-0 text-xs tabular-nums",
              isOverLimit ? "font-medium text-red-500" : "text-surface-400"
            )}
          >
            {MAX_MESSAGE_LENGTH - charCount}
          </span>
        )}
        <button
          onClick={onSend}
          disabled={isEmpty || isOverLimit || isPending}
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-primary-500 text-white transition-colors hover:bg-primary-600 disabled:opacity-40"
        >
          <Send className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

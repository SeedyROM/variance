import { cn } from "../../utils/cn";
import type { ReactionSummary } from "../../api/types";

const COMMON_EMOJIS = ["👍", "❤️", "😂", "😮", "😢", "😡", "🎉", "👎"];

interface EmojiBarProps {
  messageId: string;
  reactions: ReactionSummary[];
  onReact: (emoji: string) => void;
}

export function EmojiBar({ reactions, onReact }: EmojiBarProps) {
  const reactedEmojis = new Set(
    reactions.filter((r) => r.reacted_by_me).map((r) => r.emoji)
  );

  return (
    <div className="flex items-center gap-0.5 rounded-full border border-surface-200 bg-white px-1.5 py-1 shadow-md dark:border-surface-700 dark:bg-surface-900">
      {COMMON_EMOJIS.map((emoji) => (
        <button
          key={emoji}
          onClick={() => onReact(emoji)}
          className={cn(
            "flex h-7 w-7 items-center justify-center rounded-full text-base transition-colors hover:bg-surface-100 dark:hover:bg-surface-800",
            reactedEmojis.has(emoji) && "bg-primary-100 dark:bg-primary-900/30"
          )}
          title={emoji}
        >
          {emoji}
        </button>
      ))}
    </div>
  );
}

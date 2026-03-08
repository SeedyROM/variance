import { Users } from "lucide-react";
import { TypingDots } from "../messages/TypingIndicator";
import { cn } from "../../utils/cn";
import { relativeTime } from "../../utils/time";
import type { MlsGroupInfo } from "../../api/types";

interface GroupConversationItemProps {
  group: MlsGroupInfo;
  isActive: boolean;
  hasUnread: boolean;
  isTyping: boolean;
  onSelect: () => void;
}

export function GroupConversationItem({
  group,
  isActive,
  hasUnread,
  isTyping,
  onSelect,
}: GroupConversationItemProps) {
  return (
    <button
      onClick={onSelect}
      className={cn(
        "flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors cursor-pointer",
        isActive
          ? "bg-primary-500/10 text-primary-700 dark:text-primary-300"
          : "hover:bg-surface-200 dark:hover:bg-surface-800"
      )}
    >
      <div className="relative shrink-0 flex h-9 w-9 items-center justify-center rounded-full bg-surface-200 dark:bg-surface-700 text-surface-600 dark:text-surface-300">
        <Users className="h-4 w-4" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between gap-2">
          <p
            className={cn(
              "truncate text-sm text-surface-900 dark:text-surface-50",
              hasUnread ? "font-bold" : "font-medium"
            )}
          >
            {group.name}
          </p>
          {hasUnread && <div className="shrink-0 w-2 h-2 rounded-full bg-primary-500" />}
        </div>
        {isTyping ? (
          <span className="flex items-center gap-1.5 text-xs text-primary-500">
            <TypingDots className="text-primary-500" />
            <span>typing</span>
          </span>
        ) : (
          <p className="truncate text-xs text-surface-500">
            {group.member_count} member{group.member_count !== 1 ? "s" : ""}
            {group.last_message_timestamp ? ` · ${relativeTime(group.last_message_timestamp)}` : ""}
          </p>
        )}
      </div>
    </button>
  );
}

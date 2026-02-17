import { Avatar } from "../ui/Avatar";
import { cn } from "../../utils/cn";
import { relativeTime } from "../../utils/time";
import type { Conversation } from "../../api/types";

interface ConversationItemProps {
  conversation: Conversation;
  isActive: boolean;
  onSelect: () => void;
  onDelete: () => void;
}

export function ConversationItem({
  conversation,
  isActive,
  onSelect,
  onDelete,
}: ConversationItemProps) {
  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    if (confirm(`Delete conversation with ${conversation.peer_did}?`)) {
      onDelete();
    }
  };

  return (
    <button
      onContextMenu={handleContextMenu}
      onClick={onSelect}
      className={cn(
        "flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors",
        isActive
          ? "bg-primary-500/10 text-primary-700 dark:text-primary-300"
          : "hover:bg-surface-200 dark:hover:bg-surface-800"
      )}
    >
      <Avatar did={conversation.peer_did} size="md" />

      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium text-surface-900 dark:text-surface-50">
          {conversation.peer_did.slice(-12)}
        </p>
        <p className="truncate text-xs text-surface-500">
          {relativeTime(conversation.last_message_timestamp)}
        </p>
      </div>
    </button>
  );
}

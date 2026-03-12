import { Avatar } from "../ui/Avatar";
import { StatusDot, StatusIndicator } from "../ui/StatusIndicator";
import { TypingDots } from "../messages/TypingIndicator";
import { Tooltip } from "../ui/Tooltip";
import { cn } from "../../utils/cn";
import { relativeTime } from "../../utils/time";
import { useIdentityStore } from "../../stores/identityStore";
import { useMessagingStore } from "../../stores/messagingStore";
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
  const localDid = useIdentityStore((s) => s.did);
  const presenceMap = useMessagingStore((s) => s.presenceMap);
  const peerNames = useMessagingStore((s) => s.peerNames);
  const unreadConversations = useMessagingStore((s) => s.unreadConversations);
  const markRead = useMessagingStore((s) => s.markRead);
  const typingUsersSet = useMessagingStore((s) => s.typingUsers.get(conversation.peer_did));
  const isTyping = typingUsersSet !== undefined && typingUsersSet.size > 0;

  const isSelf = conversation.peer_did === localDid;
  const isOnline = isSelf || (presenceMap.get(conversation.peer_did) ?? false);
  const hasUnread = (conversation.has_unread ?? false) || unreadConversations.has(conversation.id);

  // Display name priority: self → backend API → WS-cached peer name → truncated DID
  const displayName = isSelf
    ? "Notes to Self"
    : (conversation.peer_username ??
      peerNames.get(conversation.peer_did) ??
      conversation.peer_did.slice(-12));

  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    if (confirm(`Delete conversation with ${displayName}?`)) {
      onDelete();
    }
  };

  const handleSelect = () => {
    onSelect();
    if (hasUnread) {
      markRead(conversation.id);
    }
  };

  const tooltipContent = (
    <div className="text-left space-y-1">
      <div className="font-semibold text-sm">{displayName}</div>
      <div className="text-[10px] text-surface-400 font-mono break-all">
        {conversation.peer_did}
      </div>
      {!isSelf && <StatusIndicator online={isOnline} size="xs" />}
    </div>
  );

  return (
    <Tooltip content={tooltipContent} placement="right" delay={600} maxWidth={300}>
      <button
        onContextMenu={handleContextMenu}
        onClick={handleSelect}
        className={cn(
          "flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors cursor-pointer",
          isActive
            ? "bg-primary-500/10 text-primary-700 dark:text-primary-300"
            : "hover:bg-surface-200 dark:hover:bg-surface-800"
        )}
      >
        {/* Avatar with online indicator sitting on the outer edge */}
        <div className="relative shrink-0">
          <Avatar did={conversation.peer_did} size="md" />
          {!isSelf && (
            <StatusDot
              online={isOnline}
              size="md"
              className={cn(
                "absolute -bottom-0.5 -right-0.5 border-2",
                isActive ? "border-primary-500/10" : "border-surface-50 dark:border-surface-900"
              )}
            />
          )}
        </div>

        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-2">
            <p
              className={cn(
                "truncate text-sm text-surface-900 dark:text-surface-50",
                hasUnread ? "font-bold" : "font-medium"
              )}
            >
              {displayName}
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
              {relativeTime(conversation.last_message_timestamp)}
            </p>
          )}
        </div>
      </button>
    </Tooltip>
  );
}

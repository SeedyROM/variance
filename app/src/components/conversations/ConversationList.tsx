import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Plus, Settings, AtSign, Copy, Check, Users, QrCode } from "lucide-react";
import { ConversationItem } from "./ConversationItem";
import { NewConversationModal } from "./NewConversationModal";
import { ChangeUsernameDialog } from "./ChangeUsernameDialog";
import { CreateGroupModal } from "./CreateGroupModal";
import { ShareContactModal } from "./ShareContactModal";
import { ThemeToggle } from "../ui/ThemeToggle";
import { ScrollArea } from "../ui/ScrollArea";
import { Avatar } from "../ui/Avatar";
import { TypingDots } from "../messages/TypingIndicator";
import { cn } from "../../utils/cn";
import { relativeTime } from "../../utils/time";
import { conversationsApi, groupsApi } from "../../api/client";
import { useMessagingStore } from "../../stores/messagingStore";
import { useIdentityStore } from "../../stores/identityStore";
import type { MlsGroupInfo } from "../../api/types";

export function ConversationList() {
  const [showNew, setShowNew] = useState(false);
  const [showNewGroup, setShowNewGroup] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showUsernameDialog, setShowUsernameDialog] = useState(false);
  const [showShareQr, setShowShareQr] = useState(false);
  const [copied, setCopied] = useState(false);

  const activeConversation = useMessagingStore((s) => s.activeConversation);
  const setActiveConversation = useMessagingStore((s) => s.setActiveConversation);
  const unreadConversations = useMessagingStore((s) => s.unreadConversations);
  const markRead = useMessagingStore((s) => s.markRead);
  const typingUsers = useMessagingStore((s) => s.typingUsers);
  const did = useIdentityStore((s) => s.did);
  const displayName = useIdentityStore((s) => s.displayName);
  const queryClient = useQueryClient();

  const { data: conversations = [] } = useQuery({
    queryKey: ["conversations"],
    queryFn: conversationsApi.list,
  });

  const { data: groups = [] } = useQuery({
    queryKey: ["groups"],
    queryFn: groupsApi.list,
  });

  const deleteMutation = useMutation({
    mutationFn: (peerDid: string) => conversationsApi.delete(peerDid),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["conversations"] });
    },
  });

  // Build a unified sorted list: DMs and groups merged by last activity.
  type ListItem =
    | {
        kind: "dm";
        id: string;
        peer_did: string;
        peer_username?: string;
        last_ts: number;
        has_unread: boolean;
      }
    | { kind: "group"; group: MlsGroupInfo; has_unread: boolean };

  const dmItems: ListItem[] = conversations.map((c) => ({
    kind: "dm",
    id: c.id,
    peer_did: c.peer_did,
    peer_username: c.peer_username,
    last_ts: c.last_message_timestamp,
    has_unread: c.has_unread ?? false,
  }));

  const groupItems: ListItem[] = groups.map((g) => ({
    kind: "group",
    group: g,
    has_unread: (g.has_unread ?? false) || unreadConversations.has(g.id),
  }));

  const allItems: ListItem[] = [...dmItems, ...groupItems].sort((a, b) => {
    const tsA = a.kind === "dm" ? a.last_ts : (a.group.last_message_timestamp ?? 0);
    const tsB = b.kind === "dm" ? b.last_ts : (b.group.last_message_timestamp ?? 0);
    return tsB - tsA;
  });

  return (
    <div className="flex h-full w-72 flex-col border-r border-surface-200 bg-surface-50 dark:border-surface-800 dark:bg-surface-900">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-surface-200 px-4 py-3 dark:border-surface-800">
        <h2 className="font-semibold text-surface-900 dark:text-surface-50 cursor-default">
          Messages
        </h2>
        <div className="flex items-center gap-1">
          <button
            onClick={() => setShowNewGroup(true)}
            className="rounded-lg p-1.5 hover:bg-surface-200 dark:hover:bg-surface-800 text-surface-500"
            title="New group"
          >
            <Users className="h-4 w-4" />
          </button>
          <button
            onClick={() => setShowNew(true)}
            className="rounded-lg p-1.5 hover:bg-surface-200 dark:hover:bg-surface-800 text-surface-500"
            title="New conversation"
          >
            <Plus className="h-4 w-4" />
          </button>
        </div>
      </div>

      {/* Unified conversation + group list */}
      <ScrollArea className="flex-1 px-2 py-1">
        {allItems.length === 0 ? (
          <div className="flex h-40 flex-col items-center justify-center gap-2 text-center cursor-default">
            <p className="text-sm text-surface-500">No conversations yet</p>
            <button
              onClick={() => setShowNew(true)}
              className="text-xs text-primary-500 hover:underline"
            >
              Start one
            </button>
          </div>
        ) : (
          allItems.map((item) => {
            if (item.kind === "dm") {
              const conv = conversations.find((c) => c.id === item.id)!;
              const isActive =
                activeConversation?.type === "dm" && activeConversation.peerId === conv.peer_did;
              return (
                <ConversationItem
                  key={conv.id}
                  conversation={conv}
                  isActive={isActive}
                  onSelect={() => {
                    setActiveConversation({ type: "dm", peerId: conv.peer_did });
                    markRead(conv.id);
                  }}
                  onDelete={() => deleteMutation.mutate(conv.peer_did)}
                />
              );
            } else {
              const g = item.group;
              const isActive =
                activeConversation?.type === "group" && activeConversation.groupId === g.id;
              const groupTypingSet = typingUsers.get(`group:${g.id}`);
              const isGroupTyping = groupTypingSet !== undefined && groupTypingSet.size > 0;
              return (
                <button
                  key={g.id}
                  onClick={() => {
                    setActiveConversation({ type: "group", groupId: g.id });
                    markRead(g.id);
                  }}
                  className={cn(
                    "flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors",
                    isActive
                      ? "bg-primary-500/10 text-primary-700 dark:text-primary-300"
                      : "hover:bg-surface-200 dark:hover:bg-surface-800"
                  )}
                >
                  {/* Group icon */}
                  <div className="relative shrink-0 flex h-9 w-9 items-center justify-center rounded-full bg-surface-200 dark:bg-surface-700 text-surface-600 dark:text-surface-300">
                    <Users className="h-4 w-4" />
                  </div>
                  <div className="min-w-0 flex-1 cursor-default">
                    <div className="flex items-center justify-between gap-2">
                      <p
                        className={cn(
                          "truncate text-sm text-surface-900 dark:text-surface-50",
                          item.has_unread ? "font-bold" : "font-medium"
                        )}
                      >
                        {g.name}
                      </p>
                      {item.has_unread && (
                        <div className="shrink-0 w-2 h-2 rounded-full bg-primary-500" />
                      )}
                    </div>
                    {isGroupTyping ? (
                      <span className="flex items-center gap-1.5 text-xs text-primary-500">
                        <TypingDots className="text-primary-500" />
                        <span>typing</span>
                      </span>
                    ) : (
                      <p className="truncate text-xs text-surface-500">
                        {g.member_count} member{g.member_count !== 1 ? "s" : ""}
                        {g.last_message_timestamp
                          ? ` · ${relativeTime(g.last_message_timestamp)}`
                          : ""}
                      </p>
                    )}
                  </div>
                </button>
              );
            }
          })
        )}
      </ScrollArea>

      {/* Footer */}
      <div className="border-t border-surface-200 px-3 py-2 dark:border-surface-800">
        <div className="flex items-center justify-between">
          <button
            onClick={() => setShowSettings(!showSettings)}
            className="flex items-center gap-2 rounded-lg p-1.5 hover:bg-surface-200 dark:hover:bg-surface-800"
          >
            {did && <Avatar did={did} size="sm" />}
            {displayName ? (
              <span className="text-xs font-medium text-surface-700 dark:text-surface-300 max-w-25 truncate">
                {displayName}
              </span>
            ) : (
              <Settings className="h-4 w-4 text-surface-500" />
            )}
          </button>

          <ThemeToggle />
        </div>

        {showSettings && did && (
          <div className="mt-2 rounded-lg bg-surface-100 p-3 dark:bg-surface-800 cursor-default space-y-2">
            {displayName && (
              <div>
                <p className="text-xs text-surface-500">Username</p>
                <p className="text-sm font-semibold text-primary-500">{displayName}</p>
              </div>
            )}
            <div>
              <p className="text-xs text-surface-500">Your DID</p>
              <p className="break-all font-mono text-xs text-surface-700 dark:text-surface-300">
                {did}
              </p>
            </div>
            <button
              onClick={() => {
                void navigator.clipboard.writeText(displayName ?? did);
                setCopied(true);
                setTimeout(() => setCopied(false), 2000);
              }}
              className="flex items-center gap-1 text-xs text-primary-500 hover:underline"
            >
              {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
              {copied ? "Copied!" : displayName ? "Copy username" : "Copy DID"}
            </button>
            <button
              onClick={() => setShowUsernameDialog(true)}
              className="flex items-center gap-1 text-xs text-primary-500 hover:underline"
            >
              <AtSign className="h-3 w-3" />
              {displayName ? "Change username" : "Set username"}
            </button>
            <button
              onClick={() => setShowShareQr(true)}
              className="flex items-center gap-1 text-xs text-primary-500 hover:underline"
            >
              <QrCode className="h-3 w-3" />
              Share contact QR
            </button>
          </div>
        )}
      </div>

      <NewConversationModal
        open={showNew}
        onClose={() => setShowNew(false)}
        onCreated={(peerId) => {
          setActiveConversation({ type: "dm", peerId });
          setShowNew(false);
        }}
      />

      <CreateGroupModal
        open={showNewGroup}
        onClose={() => setShowNewGroup(false)}
        onCreated={(groupId) => {
          setActiveConversation({ type: "group", groupId });
          setShowNewGroup(false);
          void queryClient.invalidateQueries({ queryKey: ["groups"] });
        }}
      />

      <ChangeUsernameDialog
        open={showUsernameDialog}
        onClose={() => setShowUsernameDialog(false)}
      />

      {did && (
        <ShareContactModal
          open={showShareQr}
          onClose={() => setShowShareQr(false)}
          did={did}
          displayName={displayName}
        />
      )}
    </div>
  );
}

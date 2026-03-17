import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, MessageSquare, Plus, Settings, Users } from "lucide-react";
import { ConversationItem } from "./ConversationItem";
import { GroupConversationItem } from "./GroupConversationItem";
import { InvitationsSection } from "./InvitationsSection";
import { NewConversationModal } from "./NewConversationModal";
import { CreateGroupModal } from "./CreateGroupModal";
import { SettingsModal } from "./SettingsModal";
import { ThemeToggle } from "../ui/ThemeToggle";
import { ScrollArea } from "../ui/ScrollArea";
import { Avatar } from "../ui/Avatar";
import { IconButton } from "../ui/IconButton";
import { conversationsApi, groupsApi } from "../../api/client";
import { useMessagingStore } from "../../stores/messagingStore";
import { useIdentityStore } from "../../stores/identityStore";
import { cn } from "../../utils/cn";
import type { MlsGroupInfo } from "../../api/types";

export function ConversationList({ width }: { width: number }) {
  const [showNew, setShowNew] = useState(false);
  const [showNewGroup, setShowNewGroup] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [conversationsOpen, setConversationsOpen] = useState(true);

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
    queryFn: async () => {
      const list = await conversationsApi.list();
      const unreadIds = list.filter((c) => c.has_unread).map((c) => c.id);
      useMessagingStore.getState().seedUnread(unreadIds);
      return list;
    },
  });

  const { data: groups = [] } = useQuery({
    queryKey: ["groups"],
    queryFn: async () => {
      const list = await groupsApi.list();
      const unreadIds = list.filter((g) => g.has_unread).map((g) => g.id);
      useMessagingStore.getState().seedUnread(unreadIds);
      return list;
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (peerDid: string) => conversationsApi.delete(peerDid),
    onSuccess: (_data, peerDid) => {
      void queryClient.invalidateQueries({ queryKey: ["conversations"] });
      if (activeConversation?.type === "dm" && activeConversation.peerId === peerDid) {
        setActiveConversation(null);
      }
    },
  });

  const leaveGroupMutation = useMutation({
    mutationFn: (groupId: string) => groupsApi.leave(groupId),
    onSuccess: (_data, groupId) => {
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      if (activeConversation?.type === "group" && activeConversation.groupId === groupId) {
        setActiveConversation(null);
      }
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

  // Set of peer DIDs we have existing conversations with, passed to
  // InvitationsSection so it can prioritize invites from known contacts.
  const knownPeerDids = new Set(conversations.map((c) => c.peer_did));

  return (
    <div
      className="flex h-full flex-col border-r border-surface-200 bg-surface-50 dark:border-surface-800 dark:bg-surface-900 shrink-0"
      style={{ width }}
    >
      {/* Spacer — clears macOS traffic lights (~28px) */}
      <div className="h-7 shrink-0" />

      {/* Header */}
      <div className="flex items-center justify-between border-b border-surface-200 px-4 py-3 dark:border-surface-800">
        <h2 className="font-semibold text-surface-900 dark:text-surface-50 cursor-default">
          Messages
        </h2>
        <div className="flex items-center gap-1">
          <IconButton onClick={() => setShowSettings(true)} title="Settings">
            <Settings className="h-4 w-4" />
          </IconButton>
          <IconButton onClick={() => setShowNewGroup(true)} title="New group">
            <Users className="h-4 w-4" />
          </IconButton>
          <IconButton onClick={() => setShowNew(true)} title="New conversation">
            <Plus className="h-4 w-4" />
          </IconButton>
        </div>
      </div>

      {/* Scrollable subsections */}
      <ScrollArea className="flex-1 px-2 py-1">
        <InvitationsSection knownPeerDids={knownPeerDids} />

        {/* Conversations subsection */}
        <div>
          <button
            onClick={() => setConversationsOpen((o) => !o)}
            className="flex w-full items-center gap-1.5 px-2 py-1.5 text-xs font-medium text-surface-500 uppercase tracking-wide cursor-pointer hover:text-surface-700 dark:hover:text-surface-300 transition-colors"
          >
            <ChevronDown
              className={cn("h-3 w-3 transition-transform", !conversationsOpen && "-rotate-90")}
            />
            <MessageSquare className="h-3 w-3" />
            Conversations
            {allItems.length > 0 && (
              <span className="text-surface-400 font-normal">({allItems.length})</span>
            )}
          </button>
          {conversationsOpen && (
            <>
              {allItems.length === 0 ? (
                <div className="flex my-4 flex-col items-center justify-center gap-2 text-center cursor-default">
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
                      activeConversation?.type === "dm" &&
                      activeConversation.peerId === conv.peer_did;
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
                      <GroupConversationItem
                        key={g.id}
                        group={g}
                        isActive={isActive}
                        hasUnread={item.has_unread}
                        isTyping={isGroupTyping}
                        onSelect={() => {
                          setActiveConversation({ type: "group", groupId: g.id });
                          markRead(g.id);
                        }}
                        onLeave={() => leaveGroupMutation.mutate(g.id)}
                      />
                    );
                  }
                })
              )}
            </>
          )}
        </div>
      </ScrollArea>

      {/* Footer */}
      <div className="border-t border-surface-200 px-3 py-2 dark:border-surface-800">
        <div className="flex items-center justify-between">
          <button
            onClick={() => setShowSettings(true)}
            className="flex items-center gap-2 rounded-lg p-1.5 cursor-pointer hover:bg-surface-200 dark:hover:bg-surface-800"
          >
            {did && <Avatar did={did} name={displayName ?? undefined} size="sm" />}
            {displayName ? (
              <span className="text-xs font-medium text-surface-700 dark:text-surface-300 max-w-25 truncate">
                {displayName}
              </span>
            ) : (
              <Settings className="h-4 w-4 text-surface-500" />
            )}
          </button>

          {width >= 257 && <ThemeToggle />}
        </div>
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

      <SettingsModal open={showSettings} onClose={() => setShowSettings(false)} />
    </div>
  );
}

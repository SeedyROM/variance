import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Plus, Settings, AtSign, Copy, Check } from "lucide-react";
import { ConversationItem } from "./ConversationItem";
import { NewConversationModal } from "./NewConversationModal";
import { ChangeUsernameDialog } from "./ChangeUsernameDialog";
import { ThemeToggle } from "../ui/ThemeToggle";
import { ScrollArea } from "../ui/ScrollArea";
import { Avatar } from "../ui/Avatar";
import { conversationsApi } from "../../api/client";
import { useMessagingStore } from "../../stores/messagingStore";
import { useIdentityStore } from "../../stores/identityStore";

export function ConversationList() {
  const [showNew, setShowNew] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showUsernameDialog, setShowUsernameDialog] = useState(false);
  const [copied, setCopied] = useState(false);
  const activeId = useMessagingStore((s) => s.activeConversationId);
  const setActiveId = useMessagingStore((s) => s.setActiveConversationId);
  const did = useIdentityStore((s) => s.did);
  const displayName = useIdentityStore((s) => s.displayName);
  const queryClient = useQueryClient();

  const { data: conversations = [] } = useQuery({
    queryKey: ["conversations"],
    queryFn: conversationsApi.list,
  });

  const deleteMutation = useMutation({
    mutationFn: (peerDid: string) => conversationsApi.delete(peerDid),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["conversations"] });
    },
  });

  return (
    <div className="flex h-full w-72 flex-col border-r border-surface-200 bg-surface-50 dark:border-surface-800 dark:bg-surface-900">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-surface-200 px-4 py-3 dark:border-surface-800">
        <h2 className="font-semibold text-surface-900 dark:text-surface-50 cursor-default">
          Messages
        </h2>
        <button
          onClick={() => setShowNew(true)}
          className="rounded-lg p-1.5 hover:bg-surface-200 dark:hover:bg-surface-800 text-surface-500"
          title="New conversation"
        >
          <Plus className="h-4 w-4" />
        </button>
      </div>

      {/* Conversation list */}
      <ScrollArea className="flex-1 px-2 py-1">
        {conversations.length === 0 ? (
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
          conversations.map((conv) => (
            <ConversationItem
              key={conv.id}
              conversation={conv}
              isActive={activeId === conv.id}
              onSelect={() => setActiveId(conv.id)}
              onDelete={() => deleteMutation.mutate(conv.peer_did)}
            />
          ))
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
          </div>
        )}
      </div>

      <NewConversationModal
        open={showNew}
        onClose={() => setShowNew(false)}
        onCreated={(id) => {
          setActiveId(id);
          setShowNew(false);
        }}
      />

      <ChangeUsernameDialog
        open={showUsernameDialog}
        onClose={() => setShowUsernameDialog(false)}
      />
    </div>
  );
}

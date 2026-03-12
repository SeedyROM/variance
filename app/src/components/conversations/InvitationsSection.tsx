import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, ChevronDown, Mail, X } from "lucide-react";
import { invitationsApi } from "../../api/client";
import { useMessagingStore } from "../../stores/messagingStore";
import { useToastStore } from "../../stores/toastStore";
import { cn } from "../../utils/cn";
import { relativeTime } from "../../utils/time";
import type { PendingInvitation } from "../../api/types";

interface InvitationsSectionProps {
  /** DIDs of peers we already have DM conversations with. */
  knownPeerDids: Set<string>;
}

export function InvitationsSection({ knownPeerDids }: InvitationsSectionProps) {
  const [open, setOpen] = useState(true);
  const queryClient = useQueryClient();
  const setActiveConversation = useMessagingStore((s) => s.setActiveConversation);
  const setPendingInvitationCount = useMessagingStore((s) => s.setPendingInvitationCount);
  const addToast = useToastStore((s) => s.addToast);

  const { data: invitations = [] } = useQuery({
    queryKey: ["invitations"],
    queryFn: async () => {
      const list = await invitationsApi.list();
      setPendingInvitationCount(list.length);
      return list;
    },
    staleTime: 10_000,
  });

  // Sort: known contacts first, then by timestamp descending within each group.
  const sorted = useMemo(() => {
    if (invitations.length === 0) return invitations;
    return [...invitations].sort((a, b) => {
      const aKnown = knownPeerDids.has(a.inviter_did) ? 1 : 0;
      const bKnown = knownPeerDids.has(b.inviter_did) ? 1 : 0;
      if (aKnown !== bKnown) return bKnown - aKnown;
      return b.timestamp - a.timestamp;
    });
  }, [invitations, knownPeerDids]);

  const acceptMutation = useMutation({
    mutationFn: (groupId: string) => invitationsApi.accept(groupId),
    onSuccess: (data) => {
      void queryClient.invalidateQueries({ queryKey: ["invitations"] });
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      setActiveConversation({ type: "group", groupId: data.group_id });
    },
    onError: (e) => addToast(String(e), "error"),
  });

  const declineMutation = useMutation({
    mutationFn: (groupId: string) => invitationsApi.decline(groupId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["invitations"] });
    },
    onError: (e) => addToast(String(e), "error"),
  });

  if (sorted.length === 0) return null;

  return (
    <div className="pb-1">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-1.5 px-2 py-1.5 text-xs font-medium text-surface-500 uppercase tracking-wide cursor-pointer hover:text-surface-700 dark:hover:text-surface-300 transition-colors"
      >
        <ChevronDown className={cn("h-3 w-3 transition-transform", !open && "-rotate-90")} />
        <Mail className="h-3 w-3" />
        Invitations
        <span className="text-surface-400 font-normal">({sorted.length})</span>
      </button>
      {open && (
        <div className="flex flex-col gap-0.5">
          {sorted.map((inv) => (
            <InvitationCard
              key={inv.group_id}
              invitation={inv}
              isKnownContact={knownPeerDids.has(inv.inviter_did)}
              isPending={
                (acceptMutation.isPending && acceptMutation.variables === inv.group_id) ||
                (declineMutation.isPending && declineMutation.variables === inv.group_id)
              }
              onAccept={() => acceptMutation.mutate(inv.group_id)}
              onDecline={() => declineMutation.mutate(inv.group_id)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function InvitationCard({
  invitation: inv,
  isKnownContact,
  isPending,
  onAccept,
  onDecline,
}: {
  invitation: PendingInvitation;
  isKnownContact: boolean;
  isPending: boolean;
  onAccept: () => void;
  onDecline: () => void;
}) {
  return (
    <div
      className={cn(
        "rounded-lg px-3 py-2.5 bg-primary-500/5 dark:bg-primary-500/10",
        isPending && "opacity-50"
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <p className="text-sm font-medium text-surface-900 dark:text-surface-50 truncate">
          {inv.group_name}
        </p>
        <span className="text-[10px] text-surface-400 shrink-0">{relativeTime(inv.timestamp)}</span>
      </div>
      <p className="text-xs text-surface-500 truncate mt-0.5">
        from {inv.inviter_display_name ?? inv.inviter_did.slice(-12)}
        {isKnownContact && (
          <span className="ml-1 text-primary-500 font-medium">&middot; contact</span>
        )}
        {inv.member_count > 0 && ` \u00B7 ${inv.member_count} members`}
      </p>
      <div className="flex gap-2 mt-2">
        <button
          onClick={onAccept}
          disabled={isPending}
          className="flex items-center gap-1 rounded-md px-2.5 py-1 text-xs font-medium bg-primary-500 text-white hover:bg-primary-600 disabled:opacity-50 cursor-pointer transition-colors"
        >
          <Check className="h-3 w-3" />
          Accept
        </button>
        <button
          onClick={onDecline}
          disabled={isPending}
          className="flex items-center gap-1 rounded-md px-2.5 py-1 text-xs font-medium bg-surface-200 text-surface-700 hover:bg-surface-300 dark:bg-surface-700 dark:text-surface-300 dark:hover:bg-surface-600 disabled:opacity-50 cursor-pointer transition-colors"
        >
          <X className="h-3 w-3" />
          Decline
        </button>
      </div>
    </div>
  );
}

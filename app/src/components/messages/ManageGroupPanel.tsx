import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { LogOut, Trash2, User, UserMinus } from "lucide-react";
import { Dialog } from "../ui/Dialog";
import { Button } from "../ui/Button";
import { groupsApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useMessagingStore } from "../../stores/messagingStore";
import type { MlsGroupInfo } from "../../api/types";

interface ManageGroupPanelProps {
  group: MlsGroupInfo;
  onClose: () => void;
  onLeave: () => void;
}

export function ManageGroupPanel({ group, onClose, onLeave }: ManageGroupPanelProps) {
  const [invitee, setInvitee] = useState("");
  const [confirmLeave, setConfirmLeave] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const queryClient = useQueryClient();
  const localDid = useIdentityStore((s) => s.did);
  const markRead = useMessagingStore((s) => s.markRead);

  const { data: members = [] } = useQuery({
    queryKey: ["group-members", group.id],
    queryFn: () => groupsApi.listMembers(group.id),
    staleTime: 30_000,
  });

  const inviteMutation = useMutation({
    mutationFn: () => groupsApi.invite(group.id, invitee.trim()),
    onSuccess: () => {
      setInvitee("");
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      void queryClient.invalidateQueries({ queryKey: ["group-members", group.id] });
    },
  });

  const kickMutation = useMutation({
    mutationFn: (memberDid: string) => groupsApi.removeMember(group.id, memberDid),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["group-members", group.id] });
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
    },
  });

  const leaveMutation = useMutation({
    mutationFn: () => groupsApi.leave(group.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      onLeave();
    },
  });

  const deleteMutation = useMutation({
    mutationFn: () => groupsApi.delete(group.id),
    onSuccess: () => {
      markRead(group.id);
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      onLeave();
    },
  });

  const canInvite = invitee.trim().length > 0;

  return (
    <Dialog open onClose={onClose} title={group.name}>
      <div className="flex flex-col gap-5">
        {/* Member list */}
        <div>
          <p className="text-xs font-medium text-surface-500 uppercase tracking-wide mb-2">
            Members ({members.length})
          </p>
          <div className="flex flex-col gap-1.5 max-h-48 overflow-y-auto">
            {members.map((m) => {
              const isMe = m.did === localDid;
              return (
                <div
                  key={m.did}
                  className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm text-surface-900 dark:text-surface-50 group"
                >
                  <User className="h-3.5 w-3.5 shrink-0 text-surface-400" />
                  <span className="truncate flex-1">
                    {m.display_name ?? m.did.slice(-12)}
                    {isMe && <span className="ml-1.5 text-xs text-surface-400">(you)</span>}
                  </span>
                  {!isMe && (
                    <button
                      onClick={() => kickMutation.mutate(m.did)}
                      disabled={kickMutation.isPending}
                      className="opacity-0 group-hover:opacity-100 p-1 rounded text-surface-400 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-900/20 transition-all"
                      title={`Remove ${m.display_name ?? m.did.slice(-12)}`}
                    >
                      <UserMinus className="h-3.5 w-3.5" />
                    </button>
                  )}
                </div>
              );
            })}
          </div>
          {kickMutation.error && (
            <p className="text-xs text-red-500 mt-1">{String(kickMutation.error)}</p>
          )}
        </div>

        {/* Invite */}
        <div className="flex flex-col gap-2">
          <p className="text-xs font-medium text-surface-500 uppercase tracking-wide">
            Invite member
          </p>
          <div>
            <label className="block text-xs text-surface-500 mb-1">Username or DID</label>
            <input
              type="text"
              value={invitee}
              onChange={(e) => setInvitee(e.target.value)}
              placeholder="alice or alice#0042 or did:variance:..."
              className="w-full rounded-lg border border-surface-300 bg-white px-3 py-2 text-sm
                dark:border-surface-600 dark:bg-surface-800 dark:text-surface-50
                focus:outline-none focus:ring-2 focus:ring-primary-500"
            />
          </div>
          {inviteMutation.error && (
            <p className="text-xs text-red-500">{String(inviteMutation.error)}</p>
          )}
          {inviteMutation.isSuccess && (
            <p className="text-xs text-green-600 dark:text-green-400">Invite sent.</p>
          )}
          <Button
            disabled={!canInvite || inviteMutation.isPending}
            loading={inviteMutation.isPending}
            onClick={() => inviteMutation.mutate()}
          >
            Invite
          </Button>
        </div>

        {/* Leave / Delete */}
        <div className="border-t border-surface-200 dark:border-surface-700 pt-4 flex flex-col gap-3">
          {/* Leave */}
          {!confirmLeave && !confirmDelete ? (
            <div className="flex flex-col gap-2">
              <button
                onClick={() => setConfirmLeave(true)}
                className="flex items-center gap-2 text-sm text-surface-500 hover:text-surface-700 dark:hover:text-surface-300"
              >
                <LogOut className="h-4 w-4" />
                Leave group
              </button>
              <button
                onClick={() => setConfirmDelete(true)}
                className="flex items-center gap-2 text-sm text-red-500 hover:text-red-600"
              >
                <Trash2 className="h-4 w-4" />
                Delete group
              </button>
            </div>
          ) : confirmLeave ? (
            <div className="flex flex-col gap-2">
              <p className="text-sm text-surface-700 dark:text-surface-300">
                Are you sure you want to leave this group?
              </p>
              <div className="flex gap-2">
                <Button
                  variant="secondary"
                  onClick={() => setConfirmLeave(false)}
                  disabled={leaveMutation.isPending}
                >
                  Cancel
                </Button>
                <button
                  onClick={() => leaveMutation.mutate()}
                  disabled={leaveMutation.isPending}
                  className="rounded-lg bg-surface-700 px-4 py-2 text-sm font-medium text-white hover:bg-surface-800 disabled:opacity-50"
                >
                  {leaveMutation.isPending ? "Leaving…" : "Leave"}
                </button>
              </div>
              {leaveMutation.error && (
                <p className="text-xs text-red-500">{String(leaveMutation.error)}</p>
              )}
            </div>
          ) : (
            <div className="flex flex-col gap-2">
              <p className="text-sm text-surface-700 dark:text-surface-300">
                Delete this group and all local message history?
              </p>
              <div className="flex gap-2">
                <Button
                  variant="secondary"
                  onClick={() => setConfirmDelete(false)}
                  disabled={deleteMutation.isPending}
                >
                  Cancel
                </Button>
                <button
                  onClick={() => deleteMutation.mutate()}
                  disabled={deleteMutation.isPending}
                  className="rounded-lg bg-red-500 px-4 py-2 text-sm font-medium text-white hover:bg-red-600 disabled:opacity-50"
                >
                  {deleteMutation.isPending ? "Deleting…" : "Delete"}
                </button>
              </div>
              {deleteMutation.error && (
                <p className="text-xs text-red-500">{String(deleteMutation.error)}</p>
              )}
            </div>
          )}
        </div>
      </div>
    </Dialog>
  );
}

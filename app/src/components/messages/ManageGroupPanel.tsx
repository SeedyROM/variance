import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { LogOut } from "lucide-react";
import { Dialog } from "../ui/Dialog";
import { Button } from "../ui/Button";
import { groupsApi } from "../../api/client";
import type { MlsGroupInfo } from "../../api/types";

interface ManageGroupPanelProps {
  group: MlsGroupInfo;
  onClose: () => void;
  onLeave: () => void;
}

export function ManageGroupPanel({ group, onClose, onLeave }: ManageGroupPanelProps) {
  const [invitee, setInvitee] = useState("");
  const [confirmLeave, setConfirmLeave] = useState(false);
  const queryClient = useQueryClient();

  const inviteMutation = useMutation({
    mutationFn: () => groupsApi.invite(group.id, invitee.trim()),
    onSuccess: () => {
      setInvitee("");
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

  const canInvite = invitee.trim().length > 0;

  return (
    <Dialog open onClose={onClose} title={group.name}>
      <div className="flex flex-col gap-5">
        {/* Member count */}
        <div>
          <p className="text-xs text-surface-500 mb-1">Members</p>
          <p className="text-sm text-surface-900 dark:text-surface-50">
            {group.member_count} member{group.member_count !== 1 ? "s" : ""}
          </p>
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

        {/* Leave */}
        <div className="border-t border-surface-200 dark:border-surface-700 pt-4">
          {!confirmLeave ? (
            <button
              onClick={() => setConfirmLeave(true)}
              className="flex items-center gap-2 text-sm text-red-500 hover:text-red-600"
            >
              <LogOut className="h-4 w-4" />
              Leave group
            </button>
          ) : (
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
                  className="rounded-lg bg-red-500 px-4 py-2 text-sm font-medium text-white hover:bg-red-600 disabled:opacity-50"
                >
                  {leaveMutation.isPending ? "Leaving…" : "Leave"}
                </button>
              </div>
              {leaveMutation.error && (
                <p className="text-xs text-red-500">{String(leaveMutation.error)}</p>
              )}
            </div>
          )}
        </div>
      </div>
    </Dialog>
  );
}

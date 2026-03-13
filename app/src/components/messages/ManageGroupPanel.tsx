import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowDown,
  ArrowUp,
  Clock,
  LogOut,
  Mail,
  Shield,
  Trash2,
  User,
  UserMinus,
  Users,
} from "lucide-react";
import { Dialog } from "../ui/Dialog";
import { Button } from "../ui/Button";
import { groupsApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useMessagingStore } from "../../stores/messagingStore";
import { useToastStore } from "../../stores/toastStore";
import { cn } from "../../utils/cn";
import type { MlsGroupInfo, OutboundInvitation } from "../../api/types";

interface ManageGroupPanelProps {
  group: MlsGroupInfo;
  onClose: () => void;
  onLeave: () => void;
}

type Tab = "members" | "invitations";

function RoleBadge({ role }: { role: string }) {
  if (role === "admin") {
    return (
      <span className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[10px] font-semibold bg-amber-100 text-amber-700 dark:bg-amber-900/30 dark:text-amber-400">
        <Shield className="h-2.5 w-2.5" />
        Admin
      </span>
    );
  }
  if (role === "moderator") {
    return (
      <span className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[10px] font-semibold bg-blue-100 text-blue-700 dark:bg-blue-900/30 dark:text-blue-400">
        Mod
      </span>
    );
  }
  return null;
}

/** Format remaining time until expiry as "Xm Ys". */
function formatTimeRemaining(expiresAt: number): string {
  const remaining = Math.max(0, expiresAt - Date.now());
  if (remaining === 0) return "Expired";
  const minutes = Math.floor(remaining / 60_000);
  const seconds = Math.floor((remaining % 60_000) / 1000);
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

function PendingInvitationRow({ invitation }: { invitation: OutboundInvitation }) {
  const [, setTick] = useState(0);

  // Tick every second to update the countdown.
  useEffect(() => {
    const id = setInterval(() => setTick((t) => t + 1), 1000);
    return () => clearInterval(id);
  }, []);

  const displayName = invitation.invitee_display_name ?? invitation.invitee_did.slice(-12);
  const timeLeft = formatTimeRemaining(invitation.expires_at);
  const isExpired = invitation.expires_at <= Date.now();

  return (
    <div className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm text-surface-900 dark:text-surface-50">
      <Mail className="h-3.5 w-3.5 shrink-0 text-surface-400" />
      <span className="truncate flex-1">{displayName}</span>
      <span
        className={cn(
          "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium",
          isExpired
            ? "bg-red-100 text-red-600 dark:bg-red-900/30 dark:text-red-400"
            : "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/30 dark:text-emerald-400"
        )}
      >
        <Clock className="h-2.5 w-2.5" />
        {timeLeft}
      </span>
    </div>
  );
}

export function ManageGroupPanel({ group, onClose, onLeave }: ManageGroupPanelProps) {
  const [invitee, setInvitee] = useState("");
  const [confirmLeave, setConfirmLeave] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [activeTab, setActiveTab] = useState<Tab>("members");
  const queryClient = useQueryClient();
  const localDid = useIdentityStore((s) => s.did);
  const markRead = useMessagingStore((s) => s.markRead);
  const addToast = useToastStore((s) => s.addToast);

  const isAdmin = group.your_role === "admin";

  const isModerator = group.your_role === "moderator";

  const { data: members = [] } = useQuery({
    queryKey: ["group-members", group.id],
    queryFn: () => groupsApi.listMembers(group.id),
    staleTime: 30_000,
  });

  const { data: outboundInvitations = [] } = useQuery({
    queryKey: ["outbound-invitations", group.id],
    queryFn: () => groupsApi.listOutboundInvitations(group.id),
    staleTime: 10_000,
    refetchInterval: 30_000, // Keep countdown fresh
    enabled: isAdmin,
  });

  const inviteMutation = useMutation({
    mutationFn: () => groupsApi.invite(group.id, invitee.trim()),
    onSuccess: () => {
      setInvitee("");
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      void queryClient.invalidateQueries({ queryKey: ["group-members", group.id] });
      void queryClient.invalidateQueries({ queryKey: ["outbound-invitations", group.id] });
    },
    onError: (e) => addToast(String(e), "error"),
  });

  const kickMutation = useMutation({
    mutationFn: (memberDid: string) => groupsApi.removeMember(group.id, memberDid),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["group-members", group.id] });
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
    },
    onError: (e) => addToast(String(e), "error"),
  });

  const changeRoleMutation = useMutation({
    mutationFn: ({ memberDid, newRole }: { memberDid: string; newRole: string }) =>
      groupsApi.changeRole(group.id, memberDid, newRole),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["group-members", group.id] });
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
    },
    onError: (e) => addToast(String(e), "error"),
  });

  const leaveMutation = useMutation({
    mutationFn: () => groupsApi.leave(group.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      onLeave();
    },
    onError: (e) => addToast(String(e), "error"),
  });

  const deleteMutation = useMutation({
    mutationFn: () => groupsApi.delete(group.id),
    onSuccess: () => {
      markRead(group.id);
      void queryClient.invalidateQueries({ queryKey: ["groups"] });
      onLeave();
    },
    onError: (e) => addToast(String(e), "error"),
  });

  const canInvite = invitee.trim().length > 0;

  return (
    <Dialog open onClose={onClose} title={group.name}>
      <div className="flex flex-col gap-5">
        {/* Tabs — only show if admin (non-admins only see Members) */}
        {isAdmin && (
          <div className="flex border-b border-surface-200 dark:border-surface-700">
            <button
              onClick={() => setActiveTab("members")}
              className={cn(
                "flex items-center gap-1.5 px-3 py-2 text-xs font-medium transition-colors",
                activeTab === "members"
                  ? "border-b-2 border-primary-500 text-primary-600 dark:text-primary-400"
                  : "text-surface-500 hover:text-surface-700 dark:hover:text-surface-300"
              )}
            >
              <Users className="h-3.5 w-3.5" />
              Members ({members.length})
            </button>
            <button
              onClick={() => setActiveTab("invitations")}
              className={cn(
                "flex items-center gap-1.5 px-3 py-2 text-xs font-medium transition-colors",
                activeTab === "invitations"
                  ? "border-b-2 border-primary-500 text-primary-600 dark:text-primary-400"
                  : "text-surface-500 hover:text-surface-700 dark:hover:text-surface-300"
              )}
            >
              <Mail className="h-3.5 w-3.5" />
              Pending Invitations
              {outboundInvitations.length > 0 && (
                <span className="ml-1 rounded-full bg-primary-100 px-1.5 py-0.5 text-[10px] font-semibold text-primary-700 dark:bg-primary-900/30 dark:text-primary-400">
                  {outboundInvitations.length}
                </span>
              )}
            </button>
          </div>
        )}

        {/* Members tab (also shown for non-admins without tabs) */}
        {(activeTab === "members" || !isAdmin) && (
          <>
            {/* Section header for non-admin view (no tabs) */}
            {!isAdmin && (
              <p className="text-xs font-medium text-surface-500 uppercase tracking-wide">
                Members ({members.length})
              </p>
            )}
            <div className="flex flex-col gap-1.5 max-h-48 overflow-y-auto">
              {members.map((m) => {
                const isMe = m.did === localDid;
                // Admins can kick anyone below them; moderators can kick members.
                const canKick =
                  !isMe &&
                  m.role !== "admin" &&
                  ((isAdmin && m.role !== "admin") || (isModerator && m.role === "member"));
                // Only admins can promote/demote, and only for non-self, non-admin members.
                const canPromote = isAdmin && !isMe && m.role === "member";
                const canDemote = isAdmin && !isMe && m.role === "moderator";
                return (
                  <div
                    key={m.did}
                    className={cn(
                      "flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm text-surface-900 dark:text-surface-50",
                      (canKick || canPromote || canDemote) && "group"
                    )}
                  >
                    <User className="h-3.5 w-3.5 shrink-0 text-surface-400" />
                    <span className="truncate flex-1">
                      {m.display_name ?? m.did.slice(-12)}
                      {isMe && <span className="ml-1.5 text-xs text-surface-400">(you)</span>}
                    </span>
                    <RoleBadge role={m.role} />
                    {canPromote && (
                      <button
                        onClick={() =>
                          changeRoleMutation.mutate({ memberDid: m.did, newRole: "moderator" })
                        }
                        disabled={changeRoleMutation.isPending}
                        className="opacity-0 group-hover:opacity-100 p-1 rounded text-surface-400 hover:text-blue-500 hover:bg-blue-50 dark:hover:bg-blue-900/20 transition-all"
                        title={`Promote ${m.display_name ?? m.did.slice(-12)} to moderator`}
                      >
                        <ArrowUp className="h-3.5 w-3.5" />
                      </button>
                    )}
                    {canDemote && (
                      <button
                        onClick={() =>
                          changeRoleMutation.mutate({ memberDid: m.did, newRole: "member" })
                        }
                        disabled={changeRoleMutation.isPending}
                        className="opacity-0 group-hover:opacity-100 p-1 rounded text-surface-400 hover:text-orange-500 hover:bg-orange-50 dark:hover:bg-orange-900/20 transition-all"
                        title={`Demote ${m.display_name ?? m.did.slice(-12)} to member`}
                      >
                        <ArrowDown className="h-3.5 w-3.5" />
                      </button>
                    )}
                    {canKick && (
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
          </>
        )}

        {/* Pending Invitations tab — admin only */}
        {isAdmin && activeTab === "invitations" && (
          <div className="flex flex-col gap-3">
            {outboundInvitations.length === 0 ? (
              <p className="text-sm text-surface-400 py-4 text-center">No pending invitations</p>
            ) : (
              <div className="flex flex-col gap-1.5 max-h-48 overflow-y-auto">
                {outboundInvitations.map((inv) => (
                  <PendingInvitationRow key={inv.invitee_did} invitation={inv} />
                ))}
              </div>
            )}
          </div>
        )}

        {/* Invite — admin only */}
        {isAdmin && (
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
            {inviteMutation.isSuccess && (
              <p className="text-xs text-green-600 dark:text-green-400">
                Invitation sent. Waiting for response...
              </p>
            )}
            <Button
              disabled={!canInvite || inviteMutation.isPending}
              loading={inviteMutation.isPending}
              onClick={() => inviteMutation.mutate()}
            >
              Invite
            </Button>
          </div>
        )}

        {/* Leave / Delete */}
        <div className="border-t border-surface-200 dark:border-surface-700 pt-4 flex flex-col gap-3">
          {!confirmLeave && !confirmDelete ? (
            <div className="flex flex-col gap-2">
              <button
                onClick={() => setConfirmLeave(true)}
                className="flex items-center gap-2 text-sm text-surface-500 hover:text-surface-700 dark:hover:text-surface-300"
              >
                <LogOut className="h-4 w-4" />
                Leave group
              </button>
              {isAdmin && (
                <button
                  onClick={() => setConfirmDelete(true)}
                  className="flex items-center gap-2 text-sm text-red-500 hover:text-red-600"
                >
                  <Trash2 className="h-4 w-4" />
                  Delete group
                </button>
              )}
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
                <Button
                  variant="secondary"
                  onClick={() => leaveMutation.mutate()}
                  disabled={leaveMutation.isPending}
                  loading={leaveMutation.isPending}
                >
                  Leave
                </Button>
              </div>
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
                <Button
                  variant="danger"
                  onClick={() => deleteMutation.mutate()}
                  disabled={deleteMutation.isPending}
                  loading={deleteMutation.isPending}
                >
                  Delete
                </Button>
              </div>
            </div>
          )}
        </div>
      </div>
    </Dialog>
  );
}

import { useState } from "react";
import { Shield, Users, Settings } from "lucide-react";
import { IconButton } from "../ui/IconButton";
import { ManageGroupPanel } from "./ManageGroupPanel";
import type { MlsGroupInfo } from "../../api/types";

interface GroupHeaderProps {
  group: MlsGroupInfo;
  onLeave: () => void;
  onToggleMembers?: () => void;
  membersOpen?: boolean;
}

export function GroupHeader({ group, onLeave, onToggleMembers, membersOpen }: GroupHeaderProps) {
  const [showManage, setShowManage] = useState(false);

  return (
    <>
      <div className="flex items-center gap-3 border-b border-surface-200 px-4 py-3 dark:border-surface-800">
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-surface-200 dark:bg-surface-700 text-surface-600 dark:text-surface-300">
          <Users className="h-4 w-4" />
        </div>
        <div className="cursor-default min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <p className="text-sm font-semibold text-surface-900 dark:text-surface-50 truncate">
              {group.name}
            </p>
            {group.your_role === "admin" && (
              <span className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[10px] font-semibold bg-amber-100 text-amber-700 dark:bg-amber-900/30 dark:text-amber-400 shrink-0">
                <Shield className="h-2.5 w-2.5" />
                Admin
              </span>
            )}
          </div>
          <p className="text-xs text-surface-500">
            {group.member_count} member{group.member_count !== 1 ? "s" : ""}
          </p>
        </div>
        <IconButton
          onClick={onToggleMembers}
          active={membersOpen}
          title={membersOpen ? "Hide members" : "Show members"}
        >
          <Users className="h-4 w-4" />
        </IconButton>
        <IconButton onClick={() => setShowManage(true)} title="Manage group">
          <Settings className="h-4 w-4" />
        </IconButton>
      </div>

      {showManage && (
        <ManageGroupPanel group={group} onClose={() => setShowManage(false)} onLeave={onLeave} />
      )}
    </>
  );
}

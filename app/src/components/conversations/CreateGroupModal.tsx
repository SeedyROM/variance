import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { groupsApi } from "../../api/client";
import { Dialog } from "../ui/Dialog";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";

interface CreateGroupModalProps {
  open: boolean;
  onClose: () => void;
  onCreated: (groupId: string) => void;
}

export function CreateGroupModal({ open, onClose, onCreated }: CreateGroupModalProps) {
  const [name, setName] = useState("");

  const mutation = useMutation({
    mutationFn: () => groupsApi.create(name.trim()),
    onSuccess: (data) => {
      onCreated(data.group_id);
      setName("");
    },
  });

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (name.trim()) mutation.mutate();
  };

  return (
    <Dialog open={open} onClose={onClose} title="New Group">
      <form onSubmit={handleSubmit} className="space-y-4">
        <div>
          <Input
            autoFocus
            label="Group name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. Team Alpha"
          />
        </div>

        {mutation.error && (
          <p className="text-xs text-red-500">{(mutation.error as Error).message}</p>
        )}

        <div className="flex justify-end gap-2">
          <Button variant="secondary" type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={!name.trim() || mutation.isPending}>
            {mutation.isPending ? "Creating…" : "Create"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

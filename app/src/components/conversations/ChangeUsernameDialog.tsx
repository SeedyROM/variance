import { useState } from "react";
import { Dialog } from "../ui/Dialog";
import { Input } from "../ui/Input";
import { Button } from "../ui/Button";
import { identityApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";

interface ChangeUsernameDialogProps {
  open: boolean;
  onClose: () => void;
}

export function ChangeUsernameDialog({ open, onClose }: ChangeUsernameDialogProps) {
  const currentUsername = useIdentityStore((s) => s.username);
  const setUsernameStore = useIdentityStore((s) => s.setUsername);
  const [username, setUsername] = useState(currentUsername ?? "");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isValid = /^[a-zA-Z][a-zA-Z0-9_-]{2,19}$/.test(username);
  const isChanged = username !== currentUsername;

  const handleSave = async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await identityApi.registerUsername(username);
      setUsernameStore(result.username, result.discriminator, result.display_name);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={currentUsername ? "Change Username" : "Set Username"}
    >
      <div className="flex flex-col gap-4">
        <p className="text-sm text-surface-600 dark:text-surface-400">
          {currentUsername
            ? "Change your username. You'll get a new discriminator."
            : "Choose a username so others can find you."}
        </p>

        <Input
          label="Username"
          value={username}
          onChange={(e) => setUsername(e.target.value.toLowerCase())}
          placeholder="satoshi"
          error={
            username.length > 0 && !isValid
              ? "3–20 chars, starts with a letter, letters/numbers/_/- only"
              : undefined
          }
        />

        {username && isValid && (
          <p className="text-sm text-surface-500">
            You'll be <span className="font-semibold text-primary-500">{username}#????</span>
          </p>
        )}

        {error && <p className="text-xs text-red-500">{error}</p>}

        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button disabled={!isValid || !isChanged} loading={loading} onClick={handleSave}>
            {currentUsername ? "Change" : "Set username"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

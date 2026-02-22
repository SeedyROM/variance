import { useState } from "react";
import { AtSign } from "lucide-react";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { identityApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";

interface UsernameStepProps {
  onComplete: () => void;
}

const USERNAME_RULES = [
  "3–20 characters",
  "Letters, numbers, underscores, and hyphens",
  "Must start with a letter",
];

export function UsernameStep({ onComplete }: UsernameStepProps) {
  const [username, setUsername] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const setUsernameStore = useIdentityStore((s) => s.setUsername);
  const setIsOnboarded = useIdentityStore((s) => s.setIsOnboarded);

  const isValid = /^[a-zA-Z][a-zA-Z0-9_-]{2,19}$/.test(username);

  const handleRegister = async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await identityApi.registerUsername(username);
      setUsernameStore(result.username, result.discriminator, result.display_name);
      setIsOnboarded(true);
      onComplete();
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="rounded-2xl bg-surface-50 p-8 shadow-lg dark:bg-surface-900">
      <div className="mx-auto mb-6 flex h-14 w-14 items-center justify-center rounded-full bg-primary-100 dark:bg-primary-950/40">
        <AtSign className="h-7 w-7 text-primary-600 dark:text-primary-400" />
      </div>

      <h2 className="mb-2 text-center text-xl font-bold text-surface-900 dark:text-surface-50">
        Choose a Username
      </h2>
      <p className="mb-6 text-center text-sm text-surface-600 dark:text-surface-400">
        Pick a name so others can find you. A discriminator (like{" "}
        <span className="font-mono text-primary-500">#0001</span>) will be assigned automatically.
      </p>

      <Input
        label="Username"
        value={username}
        onChange={(e) => setUsername(e.target.value.toLowerCase())}
        placeholder="satoshi"
        error={
          username.length > 0 && !isValid
            ? "Must be 3–20 chars, start with a letter, letters/numbers/_/- only"
            : undefined
        }
      />

      {username && isValid && (
        <p className="mt-2 text-sm text-surface-500">
          You'll be <span className="font-semibold text-primary-500">{username}#????</span>
        </p>
      )}

      <ul className="mt-4 space-y-1">
        {USERNAME_RULES.map((rule) => (
          <li key={rule} className="flex items-center gap-2 text-xs text-surface-500">
            <span className="text-surface-400">•</span> {rule}
          </li>
        ))}
      </ul>

      {error && (
        <div className="mt-4 rounded-lg bg-red-50 p-3 text-sm text-red-700 dark:bg-red-950/30 dark:text-red-400">
          {error}
        </div>
      )}

      <div className="mt-6 flex gap-3">
        <Button
          variant="secondary"
          className="flex-1"
          onClick={() => {
            setIsOnboarded(true);
            onComplete();
          }}
        >
          Skip for now
        </Button>
        <Button className="flex-1" disabled={!isValid} loading={loading} onClick={handleRegister}>
          Claim username
        </Button>
      </div>
    </div>
  );
}

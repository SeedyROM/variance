import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CheckCircle } from "lucide-react";
import { Button } from "../ui/Button";
import { useAppStore } from "../../stores/appStore";
import { useIdentityStore } from "../../stores/identityStore";
import { resetApiBase } from "../../api/client";

interface SetupCompleteProps {
  did: string;
  onStart: () => void;
}

export function SetupComplete({ did, onStart }: SetupCompleteProps) {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const setNodeStatus = useAppStore((s) => s.setNodeStatus);
  const setApiPort = useAppStore((s) => s.setApiPort);
  const setIsOnboarded = useIdentityStore((s) => s.setIsOnboarded);
  const identityPath = useIdentityStore((s) => s.identityPath);

  const handleStart = async () => {
    setLoading(true);
    setError(null);
    try {
      setNodeStatus("starting");
      const port = await invoke<number>("start_node", {
        identityPath: identityPath ?? (await invoke<string>("default_identity_path")),
      });
      setApiPort(port);
      resetApiBase();
      setNodeStatus("running");
      setIsOnboarded(true);
      onStart();
    } catch (e) {
      setNodeStatus("error");
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="rounded-2xl bg-surface-50 p-8 shadow-lg dark:bg-surface-900 text-center">
      <div className="mx-auto mb-6 flex h-16 w-16 items-center justify-center rounded-full bg-green-100 dark:bg-green-950/40">
        <CheckCircle className="h-8 w-8 text-green-600 dark:text-green-400" />
      </div>

      <h2 className="mb-2 text-xl font-bold text-surface-900 dark:text-surface-50">
        Identity Ready
      </h2>

      <p className="mb-6 text-sm text-surface-600 dark:text-surface-400">
        Your decentralized identity has been created.
      </p>

      <div className="mb-6 rounded-lg bg-surface-100 p-3 dark:bg-surface-800">
        <p className="mb-1 text-xs font-medium uppercase tracking-wide text-surface-500">
          Your DID
        </p>
        <p className="break-all font-mono text-xs text-primary-500">{did}</p>
      </div>

      {error && (
        <div className="mb-4 rounded-lg bg-red-50 p-3 text-sm text-red-700 dark:bg-red-950/30 dark:text-red-400">
          {error}
        </div>
      )}

      <Button className="w-full" size="lg" loading={loading} onClick={handleStart}>
        Start Variance
      </Button>
    </div>
  );
}

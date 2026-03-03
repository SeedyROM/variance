import { useState, useCallback, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Lock, Eye, EyeOff } from "lucide-react";
import { Button } from "../ui/Button";
import { useAppStore } from "../../stores/appStore";
import { useIdentityStore } from "../../stores/identityStore";
import { resetApiBase } from "../../api/client";
import { cn } from "../../utils/cn";

export function UnlockScreen() {
  const [passphrase, setPassphrase] = useState("");
  const [showPassphrase, setShowPassphrase] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const setNodeStatus = useAppStore((s) => s.setNodeStatus);
  const setApiPort = useAppStore((s) => s.setApiPort);

  const handleUnlock = useCallback(
    async (e?: FormEvent) => {
      e?.preventDefault();
      if (!passphrase.trim()) return;

      setLoading(true);
      setError(null);
      try {
        const path = await invoke<string>("default_identity_path");
        setNodeStatus("starting");
        const port = await invoke<number>("start_node", {
          identityPath: path,
          passphrase: passphrase,
        });
        setApiPort(port);
        resetApiBase();
        setNodeStatus("running");
      } catch (e) {
        setNodeStatus("needs-unlock");
        const msg = String(e);
        if (msg.includes("Decryption failed") || msg.includes("wrong passphrase")) {
          setError("Wrong passphrase. Please try again.");
        } else if (msg.includes("already starting")) {
          // React StrictMode double-mount race — ignore
          return;
        } else {
          setError(msg);
        }
      } finally {
        setLoading(false);
      }
    },
    [passphrase, setNodeStatus, setApiPort]
  );

  const handleReset = () => {
    useIdentityStore.getState().reset();
    setNodeStatus("idle");
  };

  const inputClass = cn(
    "w-full rounded-lg border pr-10 px-3 py-2 text-sm text-surface-900 placeholder:text-surface-400",
    "focus:outline-none focus:ring-2",
    "dark:text-surface-50 dark:placeholder:text-surface-600",
    error
      ? "border-red-500 focus:border-red-500 focus:ring-red-500/20 bg-surface-50 dark:bg-surface-900"
      : "border-surface-300 focus:border-primary-500 focus:ring-primary-500/20 bg-surface-50 dark:border-surface-800 dark:bg-surface-900"
  );

  return (
    <div className="flex min-h-screen items-center justify-center bg-surface-100 dark:bg-surface-950 p-4">
      <div className="w-full max-w-sm">
        <form
          onSubmit={handleUnlock}
          className="rounded-2xl bg-surface-50 p-8 shadow-lg dark:bg-surface-900"
        >
          <div className="mb-2 flex items-center justify-center">
            <div className="flex h-14 w-14 items-center justify-center rounded-full bg-primary-100 dark:bg-primary-950/40">
              <Lock className="h-7 w-7 text-primary-500" />
            </div>
          </div>

          <h2 className="mb-1 text-center text-xl font-bold text-surface-900 dark:text-surface-50">
            Welcome back
          </h2>
          <p className="mb-6 text-center text-sm text-surface-500 dark:text-surface-400">
            Enter your passphrase to unlock Variance
          </p>

          <div className="mb-4">
            <label className="mb-1 block text-sm font-medium text-surface-700 dark:text-surface-300">
              Passphrase
            </label>
            <div className="relative">
              <input
                type={showPassphrase ? "text" : "password"}
                value={passphrase}
                onChange={(e) => {
                  setPassphrase(e.target.value);
                  if (error) setError(null);
                }}
                autoFocus
                autoComplete="current-password"
                autoCorrect="off"
                autoCapitalize="none"
                spellCheck={false}
                placeholder="Enter your passphrase"
                className={inputClass}
              />
              <button
                type="button"
                onClick={() => setShowPassphrase((v) => !v)}
                className="absolute inset-y-0 right-0 flex items-center px-3 text-surface-400 hover:text-surface-600 dark:hover:text-surface-300"
                tabIndex={-1}
              >
                {showPassphrase ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
              </button>
            </div>
            {error && <p className="mt-1 text-xs text-red-500">{error}</p>}
          </div>

          <Button
            type="submit"
            className="w-full"
            size="lg"
            loading={loading}
            disabled={!passphrase.trim()}
          >
            Unlock
          </Button>

          <button
            type="button"
            onClick={handleReset}
            className="mt-4 block w-full text-center text-xs text-surface-400 hover:text-surface-600 dark:hover:text-surface-500"
          >
            Use a different identity
          </button>
        </form>
      </div>
    </div>
  );
}

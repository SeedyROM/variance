import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../ui/Button";
import { MnemonicDisplay } from "./MnemonicDisplay";
import { useIdentityStore } from "../../stores/identityStore";
import type { GeneratedIdentity } from "../../api/types";

interface GenerateStepProps {
  onBack: () => void;
  onComplete: (did: string) => void;
}

export function GenerateStep({ onBack, onComplete }: GenerateStepProps) {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [identity, setIdentity] = useState<GeneratedIdentity | null>(null);
  const setIdentityPath = useIdentityStore((s) => s.setIdentityPath);

  const handleGenerate = async () => {
    setLoading(true);
    setError(null);
    try {
      const identityPath = await invoke<string>("default_identity_path");
      const generated = await invoke<GeneratedIdentity>("generate_identity", {
        outputPath: identityPath,
      });
      setIdentityPath(identityPath);
      setIdentity(generated);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  if (!identity) {
    return (
      <div className="rounded-2xl bg-surface-50 p-8 shadow-lg dark:bg-surface-900">
        <button
          onClick={onBack}
          className="mb-6 flex items-center gap-1 text-sm text-surface-500 hover:text-surface-700 dark:hover:text-surface-300"
        >
          ← Back
        </button>

        <h2 className="mb-2 text-xl font-bold text-surface-900 dark:text-surface-50">
          Generate Identity
        </h2>
        <p className="mb-6 text-sm text-surface-600 dark:text-surface-400">
          A new cryptographic identity will be created for you.
        </p>

        {error && (
          <div className="mb-4 rounded-lg bg-red-50 p-3 text-sm text-red-700 dark:bg-red-950/30 dark:text-red-400">
            {error}
          </div>
        )}

        <Button className="w-full" loading={loading} onClick={handleGenerate}>
          Generate my identity
        </Button>
      </div>
    );
  }

  return (
    <div className="rounded-2xl bg-surface-50 p-8 shadow-lg dark:bg-surface-900">
      <h2 className="mb-2 text-xl font-bold text-surface-900 dark:text-surface-50">
        Your Recovery Phrase
      </h2>
      <p className="mb-6 text-sm text-surface-600 dark:text-surface-400">
        Your DID: <span className="font-mono text-xs text-primary-500">{identity.did}</span>
      </p>
      <MnemonicDisplay words={identity.mnemonic} onConfirmed={() => onComplete(identity.did)} />
    </div>
  );
}

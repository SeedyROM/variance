import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../ui/Button";
import { useIdentityStore } from "../../stores/identityStore";

interface RecoverStepProps {
  onBack: () => void;
  onComplete: (did: string) => void;
}

export function RecoverStep({ onBack, onComplete }: RecoverStepProps) {
  const [phrase, setPhrase] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const setIdentityPath = useIdentityStore((s) => s.setIdentityPath);

  const wordCount = phrase.trim().split(/\s+/).filter(Boolean).length;
  const isValid = wordCount === 12;

  const handleRecover = async () => {
    setLoading(true);
    setError(null);
    try {
      const identityPath = await invoke<string>("default_identity_path");
      const did = await invoke<string>("recover_identity", {
        mnemonic: phrase.trim(),
        outputPath: identityPath,
      });
      setIdentityPath(identityPath);
      onComplete(did);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="rounded-2xl bg-surface-50 p-8 shadow-lg dark:bg-surface-900">
      <button
        onClick={onBack}
        className="mb-6 flex items-center gap-1 text-sm text-surface-500 hover:text-surface-700 dark:hover:text-surface-300"
      >
        ← Back
      </button>

      <h2 className="mb-2 text-xl font-bold text-surface-900 dark:text-surface-50">
        Recover Identity
      </h2>
      <p className="mb-6 text-sm text-surface-600 dark:text-surface-400">
        Enter your 12-word recovery phrase separated by spaces.
      </p>

      <div className="mb-4 flex flex-col gap-1">
        <label className="text-sm font-medium text-surface-700 dark:text-surface-300">
          Recovery Phrase
        </label>
        <textarea
          rows={4}
          value={phrase}
          onChange={(e) => setPhrase(e.target.value)}
          placeholder="word1 word2 word3 word4 word5 word6 word7 word8 word9 word10 word11 word12"
          className="w-full resize-none rounded-lg border border-surface-300 bg-surface-50 p-3 font-mono text-sm text-surface-900 placeholder:text-surface-400 focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/20 dark:border-surface-800 dark:bg-surface-900 dark:text-surface-50"
        />
        <p className="text-xs text-surface-500">
          {wordCount} / 12 words
          {wordCount > 12 && <span className="ml-1 text-red-500">— too many words</span>}
        </p>
      </div>

      {error && (
        <div className="mb-4 rounded-lg bg-red-50 p-3 text-sm text-red-700 dark:bg-red-950/30 dark:text-red-400">
          {error}
        </div>
      )}

      <Button className="w-full" disabled={!isValid} loading={loading} onClick={handleRecover}>
        Recover identity
      </Button>
    </div>
  );
}

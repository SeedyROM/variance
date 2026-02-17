import { useState } from "react";
import { cn } from "../../utils/cn";

interface MnemonicDisplayProps {
  words: string[];
  onConfirmed: () => void;
}

export function MnemonicDisplay({ words, onConfirmed }: MnemonicDisplayProps) {
  const [confirmed, setConfirmed] = useState(false);

  return (
    <div className="flex flex-col gap-4">
      <div className="rounded-lg border border-amber-400/40 bg-amber-50 p-3 dark:bg-amber-950/30">
        <p className="text-sm font-medium text-amber-800 dark:text-amber-300">
          Write down these 12 words in order. They are the only way to recover
          your identity. Never share them or store them digitally.
        </p>
      </div>

      <div className="grid grid-cols-3 gap-2">
        {words.map((word, i) => (
          <div
            key={i}
            className={cn(
              "flex items-center gap-1.5 rounded-lg border border-surface-200 bg-surface-100 px-2 py-1.5",
              "dark:border-surface-800 dark:bg-surface-800"
            )}
          >
            <span className="w-4 text-xs text-surface-400">{i + 1}</span>
            <span className="text-sm font-mono font-medium text-surface-900 dark:text-surface-50">
              {word}
            </span>
          </div>
        ))}
      </div>

      <label className="flex cursor-pointer items-start gap-3">
        <input
          type="checkbox"
          checked={confirmed}
          onChange={(e) => setConfirmed(e.target.checked)}
          className="mt-0.5 h-4 w-4 rounded accent-primary-500"
        />
        <span className="text-sm text-surface-700 dark:text-surface-300">
          I have written down these 12 words and stored them safely.
        </span>
      </label>

      <button
        disabled={!confirmed}
        onClick={onConfirmed}
        className={cn(
          "w-full rounded-lg py-2.5 text-sm font-medium transition-colors",
          confirmed
            ? "bg-primary-500 text-white hover:bg-primary-600"
            : "cursor-not-allowed bg-surface-200 text-surface-400 dark:bg-surface-800 dark:text-surface-600"
        )}
      >
        Continue
      </button>
    </div>
  );
}

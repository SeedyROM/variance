import { useState } from "react";
import { cn } from "../../utils/cn";
import { Checkbox } from "../ui/Checkbox";

interface MnemonicDisplayProps {
  words: string[];
  onConfirmed: () => void;
}

export function MnemonicDisplay({ words, onConfirmed }: MnemonicDisplayProps) {
  const [confirmed, setConfirmed] = useState(false);

  return (
    <div className="flex flex-col gap-4">
      <div className="rounded-lg border border-red-400/60 bg-red-50 p-3 dark:bg-red-950/30">
        <p className="text-sm font-bold text-red-800 dark:text-red-300">
          This is the only time you will see these words.
        </p>
        <p className="mt-1 text-sm text-red-700 dark:text-red-400">
          Write them down on paper and store them somewhere safe. If you lose your passphrase, these
          12 words are the only way to recover your identity and message history. They cannot be
          displayed again.
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

      <Checkbox
        checked={confirmed}
        onChange={(e) => setConfirmed(e.target.checked)}
        label="I have written down these 12 words and stored them safely."
      />

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

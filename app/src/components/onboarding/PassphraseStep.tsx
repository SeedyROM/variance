import { useState } from "react";
import { Eye, EyeOff, Lock } from "lucide-react";
import { Button } from "../ui/Button";
import { cn } from "../../utils/cn";

function strength(p: string): "too-short" | "weak" | "fair" | "good" | "strong" {
  if (p.length < 3) return "too-short";
  let s = 0;
  if (p.length >= 8) s++;
  if (p.length >= 12) s++;
  if (p.length >= 16) s++;
  if (/[0-9]/.test(p)) s++;
  if (/[^a-zA-Z0-9]/.test(p)) s++;
  return s <= 1 ? "weak" : s === 2 ? "fair" : s === 3 ? "good" : "strong";
}

interface PassphraseStepProps {
  onBack: () => void;
  onSkip: () => void;
  onConfirm: (passphrase: string) => void;
}

export function PassphraseStep({ onBack, onSkip, onConfirm }: PassphraseStepProps) {
  const [passphrase, setPassphrase] = useState("");
  const [confirm, setConfirm] = useState("");
  const [showPassphrase, setShowPassphrase] = useState(false);
  const [showConfirm, setShowConfirm] = useState(false);

  const trimmed = passphrase.trim();
  const hasWhitespacePadding = passphrase.length > 0 && passphrase !== trimmed;

  let passphraseError: string | null = null;
  if (passphrase.length > 0 && trimmed.length === 0) {
    passphraseError = "Cannot be only whitespace";
  } else if (passphrase.length > 0 && trimmed.length < 3) {
    passphraseError = "Too short — minimum 3 characters";
  } else if (passphrase.length > 256) {
    passphraseError = "Too long — maximum 256 characters";
  }

  const confirmError =
    confirm.length > 0 && confirm !== passphrase ? "Passphrases don't match" : null;

  const isValid = passphraseError === null && confirmError === null && trimmed.length >= 3;

  const level = passphrase.length >= 3 ? strength(passphrase) : null;

  const segmentColor = (idx: number) => {
    if (!level || level === "too-short") return "bg-surface-200 dark:bg-surface-700";
    const filledCount = level === "weak" ? 1 : level === "fair" ? 2 : level === "good" ? 3 : 4;
    if (idx >= filledCount) return "bg-surface-200 dark:bg-surface-700";
    if (level === "weak") return "bg-red-500";
    if (level === "fair") return "bg-amber-500";
    if (level === "good") return "bg-blue-500";
    return "bg-green-500";
  };

  const inputClass = (hasError: boolean) =>
    cn(
      "w-full rounded-lg border pr-10 px-3 py-2 text-sm text-surface-900 placeholder:text-surface-400",
      "focus:outline-none focus:ring-2",
      "dark:text-surface-50 dark:placeholder:text-surface-600",
      "disabled:cursor-not-allowed disabled:opacity-50",
      hasError
        ? "border-red-500 focus:border-red-500 focus:ring-red-500/20 bg-surface-50 dark:bg-surface-900"
        : "border-surface-300 focus:border-primary-500 focus:ring-primary-500/20 bg-surface-50 dark:border-surface-800 dark:bg-surface-900"
    );

  return (
    <div className="rounded-2xl bg-surface-50 p-8 shadow-lg dark:bg-surface-900">
      <button
        onClick={onBack}
        className="mb-6 flex items-center gap-1 text-sm text-surface-500 hover:text-surface-700 dark:hover:text-surface-300"
      >
        ← Back
      </button>

      <div className="mb-2 flex items-center gap-3">
        <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-primary-100 dark:bg-primary-950/40">
          <Lock className="h-5 w-5 text-primary-500" />
        </div>
        <h2 className="text-xl font-bold text-surface-900 dark:text-surface-50">
          Protect your identity
        </h2>
      </div>

      <p className="mb-6 text-sm text-surface-600 dark:text-surface-400">
        Optional — encrypts your identity file with Argon2id + AES-256-GCM. You'll need this
        passphrase every time you start Variance.
      </p>

      <div className="mb-1 flex flex-col gap-1">
        <label className="text-sm font-medium text-surface-700 dark:text-surface-300">
          Passphrase
        </label>
        <div className="relative">
          <input
            type={showPassphrase ? "text" : "password"}
            value={passphrase}
            onChange={(e) => setPassphrase(e.target.value)}
            autoComplete="new-password"
            autoCorrect="off"
            autoCapitalize="none"
            spellCheck={false}
            className={inputClass(passphraseError !== null)}
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
        {passphraseError && <p className="text-xs text-red-500">{passphraseError}</p>}
        {!passphraseError && hasWhitespacePadding && (
          <p className="text-xs text-amber-500">Leading/trailing spaces will be trimmed</p>
        )}
      </div>

      {passphrase.length >= 3 && (
        <div className="mb-4 mt-2 flex gap-1">
          {[0, 1, 2, 3].map((i) => (
            <div
              key={i}
              className={cn("h-1.5 flex-1 rounded-full transition-colors", segmentColor(i))}
            />
          ))}
        </div>
      )}
      {passphrase.length < 3 && <div className="mb-4" />}

      <div className="mb-6 flex flex-col gap-1">
        <label className="text-sm font-medium text-surface-700 dark:text-surface-300">
          Confirm passphrase
        </label>
        <div className="relative">
          <input
            type={showConfirm ? "text" : "password"}
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            autoComplete="new-password"
            autoCorrect="off"
            autoCapitalize="none"
            spellCheck={false}
            className={inputClass(confirmError !== null)}
          />
          <button
            type="button"
            onClick={() => setShowConfirm((v) => !v)}
            className="absolute inset-y-0 right-0 flex items-center px-3 text-surface-400 hover:text-surface-600 dark:hover:text-surface-300"
            tabIndex={-1}
          >
            {showConfirm ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
          </button>
        </div>
        {confirmError && <p className="text-xs text-red-500">{confirmError}</p>}
      </div>

      <Button className="mb-3 w-full" disabled={!isValid} onClick={() => onConfirm(passphrase)}>
        Set passphrase
      </Button>
      <Button variant="ghost" className="w-full" onClick={onSkip}>
        Continue without passphrase
      </Button>
    </div>
  );
}

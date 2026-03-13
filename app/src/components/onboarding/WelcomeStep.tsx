import { Shield, RefreshCw } from "lucide-react";
import { Button } from "../ui/Button";

interface WelcomeStepProps {
  onGenerate: () => void;
  onRecover: () => void;
}

export function WelcomeStep({ onGenerate, onRecover }: WelcomeStepProps) {
  return (
    <div className="rounded-2xl bg-surface-50 p-8 shadow-lg dark:bg-surface-900 dark:border dark:border-surface-700 cursor-default">
      <div className="mb-8 text-center">
        <div className="mx-auto mb-4 flex h-16 w-16 items-center justify-center rounded-2xl bg-primary-500">
          <Shield className="h-8 w-8 text-white" />
        </div>
        <h1 className="text-2xl font-bold text-surface-900 dark:text-surface-50">
          Welcome to Variance
        </h1>
        <p className="mt-2 text-sm text-surface-600 dark:text-surface-400">
          Decentralized, private communications — you own your identity.
        </p>
      </div>

      <div className="flex flex-col gap-3">
        <Button size="lg" className="w-full" onClick={onGenerate}>
          <Shield className="h-4 w-4" />
          Create new identity
        </Button>

        <Button variant="secondary" size="lg" className="w-full" onClick={onRecover}>
          <RefreshCw className="h-4 w-4" />
          Recover existing identity
        </Button>
      </div>
    </div>
  );
}

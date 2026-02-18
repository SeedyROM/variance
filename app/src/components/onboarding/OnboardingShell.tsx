import { useState } from "react";
import { WelcomeStep } from "./WelcomeStep";
import { GenerateStep } from "./GenerateStep";
import { RecoverStep } from "./RecoverStep";
import { SetupComplete } from "./SetupComplete";

type Step = "welcome" | "generate" | "recover" | "complete";

interface OnboardingShellProps {
  onComplete: () => void;
}

export function OnboardingShell({ onComplete }: OnboardingShellProps) {
  const [step, setStep] = useState<Step>("welcome");
  const [completedDid, setCompletedDid] = useState<string | null>(null);

  const handleGenerated = (did: string) => {
    setCompletedDid(did);
    setStep("complete");
  };

  const handleRecovered = (did: string) => {
    setCompletedDid(did);
    setStep("complete");
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-surface-100 dark:bg-surface-950 p-4">
      <div className="w-full max-w-lg">
        {step === "welcome" && (
          <WelcomeStep
            onGenerate={() => setStep("generate")}
            onRecover={() => setStep("recover")}
          />
        )}
        {step === "generate" && (
          <GenerateStep onBack={() => setStep("welcome")} onComplete={handleGenerated} />
        )}
        {step === "recover" && (
          <RecoverStep onBack={() => setStep("welcome")} onComplete={handleRecovered} />
        )}
        {step === "complete" && completedDid && (
          <SetupComplete did={completedDid} onStart={onComplete} />
        )}
      </div>
    </div>
  );
}

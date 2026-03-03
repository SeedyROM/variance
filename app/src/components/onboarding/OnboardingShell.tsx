import { useState } from "react";
import { WelcomeStep } from "./WelcomeStep";
import { PassphraseStep } from "./PassphraseStep";
import { GenerateStep } from "./GenerateStep";
import { RecoverStep } from "./RecoverStep";
import { SetupComplete } from "./SetupComplete";
import { UsernameStep } from "./UsernameStep";

type Step = "welcome" | "passphrase" | "generate" | "recover" | "complete" | "username";

interface OnboardingShellProps {
  onComplete: () => void;
}

export function OnboardingShell({ onComplete }: OnboardingShellProps) {
  const [step, setStep] = useState<Step>("welcome");
  const [completedDid, setCompletedDid] = useState<string | null>(null);
  const [passphrase, setPassphrase] = useState<string | null>(null);
  const [afterPassphrase, setAfterPassphrase] = useState<"generate" | "recover">("generate");

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
            onGenerate={() => {
              setAfterPassphrase("generate");
              setStep("passphrase");
            }}
            onRecover={() => {
              setAfterPassphrase("recover");
              setStep("passphrase");
            }}
          />
        )}
        {step === "passphrase" && (
          <PassphraseStep
            onBack={() => setStep("welcome")}
            onSkip={() => {
              setPassphrase(null);
              setStep(afterPassphrase);
            }}
            onConfirm={(p) => {
              setPassphrase(p.trim() || null);
              setStep(afterPassphrase);
            }}
          />
        )}
        {step === "generate" && (
          <GenerateStep
            passphrase={passphrase}
            onBack={() => setStep("passphrase")}
            onComplete={handleGenerated}
          />
        )}
        {step === "recover" && (
          <RecoverStep
            passphrase={passphrase}
            onBack={() => setStep("passphrase")}
            onComplete={handleRecovered}
          />
        )}
        {step === "complete" && completedDid && (
          <SetupComplete
            did={completedDid}
            passphrase={passphrase}
            onStart={() => setStep("username")}
          />
        )}
        {step === "username" && <UsernameStep onComplete={onComplete} />}
      </div>
    </div>
  );
}

import { useState } from "react";
import { AtSign, Copy, Check, QrCode, Lock } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../ui/Button";
import { Avatar } from "../ui/Avatar";
import { Input } from "../ui/Input";
import { ChangeUsernameDialog } from "../conversations/ChangeUsernameDialog";
import { ShareContactModal } from "../conversations/ShareContactModal";
import { useIdentityStore } from "../../stores/identityStore";
import { useAppStore } from "../../stores/appStore";
import { useToastStore } from "../../stores/toastStore";

export function AccountSection() {
  const did = useIdentityStore((s) => s.did);
  const displayName = useIdentityStore((s) => s.displayName);
  const setNodeStatus = useAppStore((s) => s.setNodeStatus);
  const closeSettings = useAppStore((s) => s.closeSettings);
  const addToast = useToastStore((s) => s.addToast);

  const [copied, setCopied] = useState(false);
  const [showUsernameDialog, setShowUsernameDialog] = useState(false);
  const [showShareQr, setShowShareQr] = useState(false);

  // Passphrase
  const [showPassphraseSection, setShowPassphraseSection] = useState(false);
  const [currentPassphrase, setCurrentPassphrase] = useState("");
  const [newPassphrase, setNewPassphrase] = useState("");
  const [confirmPassphrase, setConfirmPassphrase] = useState("");
  const [changingPassphrase, setChangingPassphrase] = useState(false);

  async function handleChangePassphrase() {
    if (newPassphrase !== confirmPassphrase) {
      addToast("New passphrases do not match", "error");
      return;
    }
    setChangingPassphrase(true);
    try {
      await invoke("change_passphrase", {
        currentPassphrase: currentPassphrase || null,
        newPassphrase: newPassphrase || null,
      });
      addToast("Passphrase changed. Please restart the app.", "success");
      setNodeStatus("idle");
      closeSettings();
    } catch (e) {
      addToast(String(e), "error");
    } finally {
      setChangingPassphrase(false);
    }
  }

  if (!did) return null;

  return (
    <>
      <div className="space-y-8">
        <div>
          <h1 className="text-lg font-semibold text-surface-900 dark:text-surface-50">Account</h1>
          <p className="mt-1 text-sm text-surface-500">
            Manage your identity, username, and security settings.
          </p>
        </div>

        {/* Identity */}
        <section className="space-y-4">
          <h3 className="text-sm font-semibold text-surface-900 dark:text-surface-50">
            Identity
          </h3>

          <div className="flex items-center gap-4 rounded-lg border border-surface-200 bg-surface-50 p-4 dark:border-surface-800 dark:bg-surface-900">
            <Avatar did={did} name={displayName ?? undefined} size="lg" />
            <div className="min-w-0 flex-1">
              {displayName && (
                <p className="text-base font-semibold text-primary-500">{displayName}</p>
              )}
              <p className="break-all font-mono text-xs text-surface-500 dark:text-surface-400 leading-relaxed">
                {did}
              </p>
            </div>
          </div>

          <div className="flex flex-wrap gap-2">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                void navigator.clipboard.writeText(displayName ?? did);
                setCopied(true);
                setTimeout(() => setCopied(false), 2000);
              }}
            >
              {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
              {copied ? "Copied!" : displayName ? "Copy username" : "Copy DID"}
            </Button>

            <Button variant="secondary" size="sm" onClick={() => setShowShareQr(true)}>
              <QrCode className="h-3.5 w-3.5" />
              Share QR
            </Button>

            <Button variant="secondary" size="sm" onClick={() => setShowUsernameDialog(true)}>
              <AtSign className="h-3.5 w-3.5" />
              {displayName ? "Change username" : "Set username"}
            </Button>
          </div>
        </section>

        {/* Divider */}
        <hr className="border-surface-200 dark:border-surface-800" />

        {/* Security */}
        <section className="space-y-4">
          <h3 className="text-sm font-semibold text-surface-900 dark:text-surface-50">
            Security
          </h3>

          {!showPassphraseSection ? (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setShowPassphraseSection(true)}
            >
              <Lock className="h-3.5 w-3.5" />
              Change passphrase
            </Button>
          ) : (
            <div className="max-w-sm space-y-3">
              <Input
                type="password"
                label="Current passphrase"
                placeholder="Leave blank if none"
                value={currentPassphrase}
                onChange={(e) => setCurrentPassphrase(e.target.value)}
              />
              <Input
                type="password"
                label="New passphrase"
                placeholder="Leave blank to remove"
                value={newPassphrase}
                onChange={(e) => setNewPassphrase(e.target.value)}
              />
              <Input
                type="password"
                label="Confirm new passphrase"
                placeholder="Re-enter new passphrase"
                value={confirmPassphrase}
                onChange={(e) => setConfirmPassphrase(e.target.value)}
              />
              <p className="text-xs text-surface-400 italic">
                The app will restart after changing the passphrase.
              </p>
              <div className="flex gap-2">
                <Button
                  size="sm"
                  onClick={handleChangePassphrase}
                  disabled={changingPassphrase}
                  loading={changingPassphrase}
                >
                  Change
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setShowPassphraseSection(false);
                    setCurrentPassphrase("");
                    setNewPassphrase("");
                    setConfirmPassphrase("");
                  }}
                >
                  Cancel
                </Button>
              </div>
            </div>
          )}
        </section>
      </div>

      <ChangeUsernameDialog
        open={showUsernameDialog}
        onClose={() => setShowUsernameDialog(false)}
      />

      {did && (
        <ShareContactModal
          open={showShareQr}
          onClose={() => setShowShareQr(false)}
          did={did}
          displayName={displayName}
        />
      )}
    </>
  );
}

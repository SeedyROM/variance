import { useState, useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AtSign, Copy, Check, QrCode, Trash2, Lock } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { Dialog } from "../ui/Dialog";
import { Button } from "../ui/Button";
import { IconButton } from "../ui/IconButton";
import { Avatar } from "../ui/Avatar";
import { Input } from "../ui/Input";
import { ChangeUsernameDialog } from "./ChangeUsernameDialog";
import { ShareContactModal } from "./ShareContactModal";
import { configApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useAppStore } from "../../stores/appStore";
import { useToastStore } from "../../stores/toastStore";
import type { RelayPeer } from "../../api/types";

interface SettingsModalProps {
  open: boolean;
  onClose: () => void;
}

export function SettingsModal({ open, onClose }: SettingsModalProps) {
  const did = useIdentityStore((s) => s.did);
  const displayName = useIdentityStore((s) => s.displayName);

  const [copied, setCopied] = useState(false);
  const [showUsernameDialog, setShowUsernameDialog] = useState(false);
  const [showShareQr, setShowShareQr] = useState(false);

  // Relay form state
  const [relayPeerId, setRelayPeerId] = useState("");
  const [relayMultiaddr, setRelayMultiaddr] = useState("");
  // null = no pending edits (show saved); non-null = local draft
  const [pendingRelays, setPendingRelays] = useState<RelayPeer[] | null>(null);
  const [saving, setSaving] = useState(false);

  const setNodeStatus = useAppStore((s) => s.setNodeStatus);
  const queryClient = useQueryClient();
  const addToast = useToastStore((s) => s.addToast);

  // Passphrase change state
  const [showPassphraseSection, setShowPassphraseSection] = useState(false);
  const [currentPassphrase, setCurrentPassphrase] = useState("");
  const [newPassphrase, setNewPassphrase] = useState("");
  const [confirmPassphrase, setConfirmPassphrase] = useState("");
  const [changingPassphrase, setChangingPassphrase] = useState(false);

  const { data: savedRelays = [] } = useQuery({
    queryKey: ["relays"],
    queryFn: configApi.getRelays,
    enabled: open,
  });

  const { data: retention } = useQuery({
    queryKey: ["retention"],
    queryFn: configApi.getRetention,
    enabled: open,
  });

  // Reset draft when modal closes
  useEffect(() => {
    if (!open) {
      setPendingRelays(null);
      setRelayPeerId("");
      setRelayMultiaddr("");
      setShowPassphraseSection(false);
      setCurrentPassphrase("");
      setNewPassphrase("");
      setConfirmPassphrase("");
    }
  }, [open]);

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
      onClose();
    } catch (e) {
      addToast(String(e), "error");
    } finally {
      setChangingPassphrase(false);
    }
  }

  const relays = pendingRelays ?? savedRelays;
  const isDirty = pendingRelays !== null;

  function handleAddToList() {
    if (!relayPeerId || !relayMultiaddr) return;
    setPendingRelays([...relays, { peer_id: relayPeerId, multiaddr: relayMultiaddr }]);
    setRelayPeerId("");
    setRelayMultiaddr("");
  }

  function handleRemoveRelay(peerId: string) {
    setPendingRelays(relays.filter((r) => r.peer_id !== peerId));
  }

  function handleRestoreDefaults() {
    setPendingRelays([]);
  }

  async function handleRetentionChange(days: number) {
    try {
      await configApi.setRetention({ group_message_max_age_days: days });
      await queryClient.invalidateQueries({ queryKey: ["retention"] });
    } catch (e) {
      addToast(String(e), "error");
    }
  }

  async function handleSave() {
    if (!isDirty) return;
    setSaving(true);
    try {
      for (const r of savedRelays) {
        await configApi.removeRelay(r.peer_id);
      }
      for (const r of pendingRelays!) {
        await configApi.addRelay(r);
      }
      await queryClient.invalidateQueries({ queryKey: ["relays"] });
      setPendingRelays(null);
    } catch (e) {
      addToast(String(e), "error");
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <Dialog open={open} onClose={onClose} title="Settings" className="max-w-lg">
        {did && (
          <div className="space-y-6">
            {/* Identity */}
            <section className="space-y-3">
              <h3 className="text-xs font-semibold uppercase tracking-wider text-surface-400 dark:text-surface-500">
                Identity
              </h3>

              <div className="flex items-center gap-3">
                <Avatar did={did} name={displayName ?? undefined} size="md" />
                <div className="min-w-0">
                  {displayName && <p className="font-semibold text-primary-500">{displayName}</p>}
                  <p className="break-all font-mono text-xs text-surface-500 dark:text-surface-400 leading-relaxed">
                    {did}
                  </p>
                </div>
              </div>

              <div className="flex flex-wrap gap-2">
                <Button
                  variant="secondary"
                  size="xs"
                  onClick={() => {
                    void navigator.clipboard.writeText(displayName ?? did);
                    setCopied(true);
                    setTimeout(() => setCopied(false), 2000);
                  }}
                >
                  {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
                  {copied ? "Copied!" : displayName ? "Copy username" : "Copy DID"}
                </Button>

                <Button variant="secondary" size="xs" onClick={() => setShowShareQr(true)}>
                  <QrCode className="h-3.5 w-3.5" />
                  Share QR
                </Button>

                <Button variant="secondary" size="xs" onClick={() => setShowUsernameDialog(true)}>
                  <AtSign className="h-3.5 w-3.5" />
                  {displayName ? "Change username" : "Set username"}
                </Button>
              </div>
            </section>

            {/* Security */}
            <section className="space-y-3">
              <h3 className="text-xs font-semibold uppercase tracking-wider text-surface-400 dark:text-surface-500">
                Security
              </h3>
              {!showPassphraseSection ? (
                <Button
                  variant="secondary"
                  size="xs"
                  onClick={() => setShowPassphraseSection(true)}
                >
                  <Lock className="h-3.5 w-3.5" />
                  Change passphrase
                </Button>
              ) : (
                <div className="space-y-2">
                  <Input
                    type="password"
                    placeholder="Current passphrase (leave blank if none)"
                    value={currentPassphrase}
                    onChange={(e) => setCurrentPassphrase(e.target.value)}
                  />
                  <Input
                    type="password"
                    placeholder="New passphrase (leave blank to remove)"
                    value={newPassphrase}
                    onChange={(e) => setNewPassphrase(e.target.value)}
                  />
                  <Input
                    type="password"
                    placeholder="Confirm new passphrase"
                    value={confirmPassphrase}
                    onChange={(e) => setConfirmPassphrase(e.target.value)}
                  />
                  <p className="text-xs text-surface-400 italic">
                    The app will restart after changing the passphrase.
                  </p>
                  <div className="flex gap-2">
                    <Button
                      size="xs"
                      onClick={handleChangePassphrase}
                      disabled={changingPassphrase}
                      loading={changingPassphrase}
                    >
                      Change
                    </Button>
                    <Button
                      variant="ghost"
                      size="xs"
                      onClick={() => setShowPassphraseSection(false)}
                    >
                      Cancel
                    </Button>
                  </div>
                </div>
              )}
            </section>

            {/* Relay Servers */}
            <section className="space-y-3">
              <h3 className="text-xs font-semibold uppercase tracking-wider text-surface-400 dark:text-surface-500">
                Relay Servers
              </h3>

              {relays.length === 0 ? (
                <p className="text-xs text-surface-400 italic">No relay servers configured.</p>
              ) : (
                <ul className="space-y-1.5">
                  {relays.map((relay) => (
                    <li
                      key={relay.peer_id}
                      className="flex items-start justify-between gap-2 rounded-md bg-surface-100 dark:bg-surface-800 px-3 py-2"
                    >
                      <div className="min-w-0">
                        <p className="font-mono text-xs text-surface-700 dark:text-surface-300 truncate">
                          {relay.peer_id}
                        </p>
                        <p className="font-mono text-xs text-surface-400 truncate">
                          {relay.multiaddr}
                        </p>
                      </div>
                      <IconButton
                        onClick={() => handleRemoveRelay(relay.peer_id)}
                        className="shrink-0 mt-0.5 hover:text-red-500 dark:hover:text-red-400 hover:bg-red-50 dark:hover:bg-red-900/20"
                        title="Remove"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </IconButton>
                    </li>
                  ))}
                </ul>
              )}

              <div className="space-y-1.5">
                <Input
                  value={relayPeerId}
                  onChange={(e) => setRelayPeerId(e.target.value)}
                  placeholder="Peer ID"
                  className="rounded-md px-2.5 py-1.5 text-xs"
                />
                <Input
                  value={relayMultiaddr}
                  onChange={(e) => setRelayMultiaddr(e.target.value)}
                  placeholder="Multiaddr (e.g. /ip4/1.2.3.4/tcp/4001)"
                  className="rounded-md px-2.5 py-1.5 text-xs"
                />
                <Button
                  variant="secondary"
                  size="xs"
                  onClick={handleAddToList}
                  disabled={!relayPeerId || !relayMultiaddr}
                  className="w-full"
                >
                  Add to list
                </Button>
              </div>

              <p className="text-xs text-surface-400 italic">
                Changes take effect after restarting the app.
              </p>

              <div className="flex items-center justify-between pt-1">
                <Button variant="ghost" size="xs" onClick={handleRestoreDefaults}>
                  Restore defaults
                </Button>
                <Button
                  size="xs"
                  onClick={handleSave}
                  disabled={!isDirty || saving}
                  loading={saving}
                >
                  Save
                </Button>
              </div>
            </section>

            {/* Message History */}
            <section className="space-y-3">
              <h3 className="text-xs font-semibold uppercase tracking-wider text-surface-400 dark:text-surface-500">
                Message History
              </h3>

              <div className="flex items-center justify-between gap-3">
                <label
                  htmlFor="retention-select"
                  className="text-xs text-surface-700 dark:text-surface-300"
                >
                  Keep messages for
                </label>
                <select
                  id="retention-select"
                  value={retention?.group_message_max_age_days ?? 30}
                  onChange={(e) => void handleRetentionChange(Number(e.target.value))}
                  className="rounded-md px-2 py-1 text-xs bg-surface-50 dark:bg-surface-900 border border-surface-300 dark:border-surface-600 text-surface-900 dark:text-surface-100 focus:outline-none focus:ring-1 focus:ring-primary-500"
                >
                  <option value={0}>Keep forever</option>
                  <option value={90}>90 days</option>
                  <option value={30}>30 days (default)</option>
                  <option value={14}>14 days</option>
                </select>
              </div>

              <p className="text-xs text-surface-400 italic">
                Applies to both direct and group messages locally stored on this device.
              </p>
            </section>
          </div>
        )}
      </Dialog>

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

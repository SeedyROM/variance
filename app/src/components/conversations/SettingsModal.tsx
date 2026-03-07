import { useState, useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AtSign, Copy, Check, QrCode, Trash2 } from "lucide-react";
import { Dialog } from "../ui/Dialog";
import { Avatar } from "../ui/Avatar";
import { ChangeUsernameDialog } from "./ChangeUsernameDialog";
import { ShareContactModal } from "./ShareContactModal";
import { configApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
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

  const queryClient = useQueryClient();

  const { data: savedRelays = [] } = useQuery({
    queryKey: ["relays"],
    queryFn: configApi.getRelays,
    enabled: open,
  });

  // Reset draft when modal closes
  useEffect(() => {
    if (!open) {
      setPendingRelays(null);
      setRelayPeerId("");
      setRelayMultiaddr("");
    }
  }, [open]);

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
                <Avatar did={did} size="md" />
                <div className="min-w-0">
                  {displayName && <p className="font-semibold text-primary-500">{displayName}</p>}
                  <p className="break-all font-mono text-xs text-surface-500 dark:text-surface-400 leading-relaxed">
                    {did}
                  </p>
                </div>
              </div>

              <div className="flex flex-wrap gap-2">
                <button
                  onClick={() => {
                    void navigator.clipboard.writeText(displayName ?? did);
                    setCopied(true);
                    setTimeout(() => setCopied(false), 2000);
                  }}
                  className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs bg-surface-100 hover:bg-surface-200 dark:bg-surface-800 dark:hover:bg-surface-700 text-surface-700 dark:text-surface-300 transition-colors"
                >
                  {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
                  {copied ? "Copied!" : displayName ? "Copy username" : "Copy DID"}
                </button>

                <button
                  onClick={() => setShowShareQr(true)}
                  className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs bg-surface-100 hover:bg-surface-200 dark:bg-surface-800 dark:hover:bg-surface-700 text-surface-700 dark:text-surface-300 transition-colors"
                >
                  <QrCode className="h-3.5 w-3.5" />
                  Share QR
                </button>

                <button
                  onClick={() => setShowUsernameDialog(true)}
                  className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs bg-surface-100 hover:bg-surface-200 dark:bg-surface-800 dark:hover:bg-surface-700 text-surface-700 dark:text-surface-300 transition-colors"
                >
                  <AtSign className="h-3.5 w-3.5" />
                  {displayName ? "Change username" : "Set username"}
                </button>
              </div>
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
                      <button
                        onClick={() => handleRemoveRelay(relay.peer_id)}
                        className="shrink-0 mt-0.5 text-surface-400 hover:text-red-500 transition-colors"
                        title="Remove"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </li>
                  ))}
                </ul>
              )}

              <div className="space-y-1.5">
                <input
                  value={relayPeerId}
                  onChange={(e) => setRelayPeerId(e.target.value)}
                  placeholder="Peer ID"
                  className="w-full rounded-md px-2.5 py-1.5 text-xs bg-surface-50 dark:bg-surface-900 border border-surface-300 dark:border-surface-600 text-surface-900 dark:text-surface-100 placeholder-surface-400 focus:outline-none focus:ring-1 focus:ring-primary-500"
                />
                <input
                  value={relayMultiaddr}
                  onChange={(e) => setRelayMultiaddr(e.target.value)}
                  placeholder="Multiaddr (e.g. /ip4/1.2.3.4/tcp/4001)"
                  className="w-full rounded-md px-2.5 py-1.5 text-xs bg-surface-50 dark:bg-surface-900 border border-surface-300 dark:border-surface-600 text-surface-900 dark:text-surface-100 placeholder-surface-400 focus:outline-none focus:ring-1 focus:ring-primary-500"
                />
                <button
                  onClick={handleAddToList}
                  disabled={!relayPeerId || !relayMultiaddr}
                  className="w-full rounded-md px-2.5 py-1.5 text-xs bg-surface-100 hover:bg-surface-200 dark:bg-surface-800 dark:hover:bg-surface-700 text-surface-700 dark:text-surface-300 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                >
                  Add to list
                </button>
              </div>

              <p className="text-xs text-surface-400 italic">
                Changes take effect after restarting the app.
              </p>

              <div className="flex items-center justify-between pt-1">
                <button
                  onClick={handleRestoreDefaults}
                  className="text-xs text-surface-500 hover:text-surface-700 dark:hover:text-surface-300 transition-colors"
                >
                  Restore defaults
                </button>
                <button
                  onClick={handleSave}
                  disabled={!isDirty || saving}
                  className="rounded-md px-3.5 py-1.5 text-xs font-medium bg-primary-500 text-white hover:bg-primary-600 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                >
                  {saving ? "Saving…" : "Save"}
                </button>
              </div>
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

import { useState } from "react";
import { Trash2, RotateCcw } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "../ui/Button";
import { IconButton } from "../ui/IconButton";
import { Input } from "../ui/Input";
import { ConfirmDialog } from "../ui/ConfirmDialog";
import { configApi } from "../../api/client";
import { useToastStore } from "../../stores/toastStore";

export function NetworkSection() {
  const queryClient = useQueryClient();
  const addToast = useToastStore((s) => s.addToast);

  const [relayPeerId, setRelayPeerId] = useState("");
  const [relayMultiaddr, setRelayMultiaddr] = useState("");
  const [addingRelay, setAddingRelay] = useState(false);
  const [showResetConfirm, setShowResetConfirm] = useState(false);
  const [resetting, setResetting] = useState(false);

  const { data: relays = [] } = useQuery({
    queryKey: ["relays"],
    queryFn: configApi.getRelays,
  });

  async function handleAddRelay() {
    if (!relayPeerId || !relayMultiaddr) return;
    setAddingRelay(true);
    try {
      await configApi.addRelay({ peer_id: relayPeerId, multiaddr: relayMultiaddr });
      await queryClient.invalidateQueries({ queryKey: ["relays"] });
      setRelayPeerId("");
      setRelayMultiaddr("");
    } catch (e) {
      addToast(String(e), "error");
    } finally {
      setAddingRelay(false);
    }
  }

  async function handleRemoveRelay(peerId: string) {
    try {
      await configApi.removeRelay(peerId);
      await queryClient.invalidateQueries({ queryKey: ["relays"] });
    } catch (e) {
      addToast(String(e), "error");
    }
  }

  async function handleRestoreDefaults() {
    setResetting(true);
    try {
      for (const r of relays) {
        await configApi.removeRelay(r.peer_id);
      }
      await queryClient.invalidateQueries({ queryKey: ["relays"] });
      setShowResetConfirm(false);
    } catch (e) {
      addToast(String(e), "error");
    } finally {
      setResetting(false);
    }
  }

  return (
    <>
      <div className="space-y-8">
        <div className="flex items-start justify-between">
          <div>
            <h1 className="text-lg font-semibold text-surface-900 dark:text-surface-50">
              Network
            </h1>
            <p className="mt-1 text-sm text-surface-500">
              Configure relay servers for offline message delivery.
            </p>
          </div>
          {relays.length > 0 && (
            <Button
              variant="ghost"
              size="xs"
              onClick={() => setShowResetConfirm(true)}
              className="shrink-0"
            >
              <RotateCcw className="h-3.5 w-3.5" />
              Restore defaults
            </Button>
          )}
        </div>

        {/* Relay Servers */}
        <section className="space-y-4">
          <h3 className="text-sm font-semibold text-surface-900 dark:text-surface-50">
            Relay Servers
          </h3>

          {relays.length === 0 ? (
            <p className="text-sm text-surface-400 italic">No relay servers configured.</p>
          ) : (
            <ul className="space-y-2">
              {relays.map((relay) => (
                <li
                  key={relay.peer_id}
                  className="flex items-start justify-between gap-3 rounded-lg border border-surface-200 bg-surface-50 px-4 py-3 dark:border-surface-800 dark:bg-surface-900"
                >
                  <div className="min-w-0">
                    <p className="font-mono text-sm text-surface-700 dark:text-surface-300 truncate">
                      {relay.peer_id}
                    </p>
                    <p className="font-mono text-xs text-surface-400 truncate">
                      {relay.multiaddr}
                    </p>
                  </div>
                  <IconButton
                    onClick={() => void handleRemoveRelay(relay.peer_id)}
                    className="shrink-0 mt-0.5 hover:text-red-500 dark:hover:text-red-400 hover:bg-red-50 dark:hover:bg-red-900/20"
                    title="Remove relay"
                  >
                    <Trash2 className="h-4 w-4" />
                  </IconButton>
                </li>
              ))}
            </ul>
          )}

          {/* Add form */}
          <div className="max-w-md space-y-3">
            <h4 className="text-xs font-medium text-surface-500 uppercase tracking-wide">
              Add relay
            </h4>
            <Input
              value={relayPeerId}
              onChange={(e) => setRelayPeerId(e.target.value)}
              placeholder="Peer ID"
            />
            <Input
              value={relayMultiaddr}
              onChange={(e) => setRelayMultiaddr(e.target.value)}
              placeholder="Multiaddr (e.g. /ip4/1.2.3.4/tcp/4001)"
            />
            <Button
              variant="secondary"
              size="sm"
              onClick={() => void handleAddRelay()}
              disabled={!relayPeerId || !relayMultiaddr || addingRelay}
              loading={addingRelay}
            >
              Add relay
            </Button>
          </div>

          <p className="text-xs text-surface-400 italic">
            Changes take effect after restarting the app.
          </p>
        </section>
      </div>

      <ConfirmDialog
        open={showResetConfirm}
        onClose={() => setShowResetConfirm(false)}
        onConfirm={() => void handleRestoreDefaults()}
        title="Restore Defaults"
        message="This will remove all configured relay servers. You can add them back later."
        confirmLabel="Remove all"
        destructive
        loading={resetting}
      />
    </>
  );
}

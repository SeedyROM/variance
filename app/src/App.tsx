import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { OnboardingShell } from "./components/onboarding/OnboardingShell";
import { ConversationList } from "./components/conversations/ConversationList";
import { MessageView } from "./components/messages/MessageView";
import { useWebSocket } from "./hooks/useWebSocket";
import { useNodeReady } from "./hooks/useNodeReady";
import { useIdentityStore } from "./stores/identityStore";
import { useAppStore } from "./stores/appStore";
import { useMessagingStore } from "./stores/messagingStore";
import { useQuery } from "@tanstack/react-query";
import { identityApi } from "./api/client";
import { resetApiBase } from "./api/client";

function LoadingScreen() {
  return (
    <div className="flex h-screen items-center justify-center bg-surface-100 dark:bg-surface-950">
      <div className="flex flex-col items-center gap-3">
        <svg className="h-8 w-8 animate-spin text-primary-500" fill="none" viewBox="0 0 24 24">
          <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
          <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
        </svg>
        <p className="text-sm text-surface-500">Starting Variance…</p>
      </div>
    </div>
  );
}

function MainShell() {
  const activeId = useMessagingStore((s) => s.activeConversationId);
  const setIdentity = useIdentityStore((s) => s.setIdentity);

  // Sync identity into store after node starts
  useQuery({
    queryKey: ["identity"],
    queryFn: async () => {
      const id = await identityApi.get();
      setIdentity(id.did, id.verifying_key, id.created_at);
      return id;
    },
  });

  // Wire up WebSocket
  useWebSocket();

  // Find the peer DID for the active conversation
  const [activePeerDid, setActivePeerDid] = useState<string | null>(null);
  useEffect(() => {
    if (!activeId) {
      setActivePeerDid(null);
      return;
    }
    // conversation_id is "sorted_did1:sorted_did2", peer is the non-local half
    // We re-query conversations to resolve, but simplify: use the stored conversations
    setActivePeerDid(null); // resolved below via the conversations query
  }, [activeId]);

  useQuery({
    queryKey: ["conversations"],
    queryFn: async () => {
      const { conversationsApi } = await import("./api/client");
      return conversationsApi.list();
    },
    enabled: true,
    select: (convs) => {
      if (activeId) {
        const active = convs.find((c) => c.id === activeId);
        if (active) setActivePeerDid(active.peer_did);
      }
      return convs;
    },
  });

  return (
    <div className="flex h-screen bg-surface-100 dark:bg-surface-950">
      <ConversationList />
      <main className="flex-1 overflow-hidden">
        {activePeerDid ? (
          <MessageView peerDid={activePeerDid} />
        ) : (
          <div className="flex h-full items-center justify-center">
            <p className="text-sm text-surface-400">
              Select a conversation or start a new one
            </p>
          </div>
        )}
      </main>
    </div>
  );
}

export function App() {
  const isOnboarded = useIdentityStore((s) => s.isOnboarded);
  const nodeStatus = useAppStore((s) => s.nodeStatus);
  const setNodeStatus = useAppStore((s) => s.setNodeStatus);
  const setApiPort = useAppStore((s) => s.setApiPort);
  const identityPath = useIdentityStore((s) => s.identityPath);

  // Poll for node readiness when starting
  useNodeReady();

  // On app launch: if onboarded, auto-start the node
  useEffect(() => {
    if (!isOnboarded) return;
    if (nodeStatus !== "idle") return;

    const startNode = async () => {
      setNodeStatus("starting");
      try {
        const path =
          identityPath ?? (await invoke<string>("default_identity_path"));
        const port = await invoke<number>("start_node", {
          identityPath: path,
        });
        setApiPort(port);
        resetApiBase();
        setNodeStatus("running");
      } catch (e) {
        setNodeStatus("error");
        console.error("Failed to start node:", e);
      }
    };

    void startNode();
  }, [isOnboarded]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!isOnboarded) {
    return (
      <OnboardingShell
        onComplete={() => {
          // isOnboarded is set in the store during onboarding; just re-render
        }}
      />
    );
  }

  if (nodeStatus === "starting" || nodeStatus === "idle") {
    return <LoadingScreen />;
  }

  if (nodeStatus === "error") {
    return (
      <div className="flex h-screen items-center justify-center bg-surface-100 dark:bg-surface-950 p-4">
        <div className="rounded-xl bg-red-50 p-6 dark:bg-red-950/30 text-center max-w-sm">
          <p className="font-semibold text-red-700 dark:text-red-400">Failed to start node</p>
          <p className="mt-2 text-sm text-red-600 dark:text-red-500">
            Check your identity file and try again.
          </p>
          <button
            onClick={() => setNodeStatus("idle")}
            className="mt-4 text-sm text-primary-500 hover:underline"
          >
            Retry
          </button>
        </div>
      </div>
    );
  }

  return <MainShell />;
}

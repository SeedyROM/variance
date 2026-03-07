import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { OnboardingShell } from "./components/onboarding/OnboardingShell";
import { UnlockScreen } from "./components/onboarding/UnlockScreen";
import { ConversationList } from "./components/conversations/ConversationList";
import { MessageView } from "./components/messages/MessageView";
import { GroupView } from "./components/messages/GroupView";
import { useWebSocket } from "./hooks/useWebSocket";
import { usePresencePolling } from "./hooks/usePresencePolling";
import { useNodeReady } from "./hooks/useNodeReady";
import { useIdentityStore } from "./stores/identityStore";
import { useAppStore } from "./stores/appStore";
import { useMessagingStore } from "./stores/messagingStore";
import { useQuery } from "@tanstack/react-query";
import { identityApi, conversationsApi, resetApiBase } from "./api/client";

function LoadingScreen() {
  return (
    <div className="flex h-screen items-center justify-center bg-surface-100 dark:bg-surface-950 overscroll-none">
      <div className="flex flex-col items-center gap-3">
        <svg className="h-8 w-8 animate-spin text-primary-500" fill="none" viewBox="0 0 24 24">
          <circle
            className="opacity-25"
            cx="12"
            cy="12"
            r="10"
            stroke="currentColor"
            strokeWidth="4"
          />
          <path
            className="opacity-75"
            fill="currentColor"
            d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
          />
        </svg>
        <p className="text-sm text-surface-500">Starting Variance…</p>
      </div>
    </div>
  );
}

const DRAG_ZONE_HEIGHT = 28; // matches the h-7 spacer in ConversationList

function MainShell() {
  const activeConversation = useMessagingStore((s) => s.activeConversation);

  useEffect(() => {
    const onMouseDown = (e: MouseEvent) => {
      if (e.button !== 0) return;
      if (e.clientY > DRAG_ZONE_HEIGHT) return;
      const el = e.target as HTMLElement;
      if (el.closest('button, a, input, textarea, [role="button"]')) return;
      void getCurrentWebviewWindow().startDragging();
    };
    document.addEventListener("mousedown", onMouseDown);
    return () => document.removeEventListener("mousedown", onMouseDown);
  }, []);
  const setIdentity = useIdentityStore((s) => s.setIdentity);
  const setUsernameStore = useIdentityStore((s) => s.setUsername);

  // Sync identity into store after node starts
  useQuery({
    queryKey: ["identity"],
    queryFn: async () => {
      const id = await identityApi.get();
      setIdentity(id.did, id.verifying_key, id.created_at);
      if (id.username && id.discriminator != null && id.display_name) {
        setUsernameStore(id.username, id.discriminator, id.display_name);
      } else {
        useIdentityStore.getState().clearUsername();
      }
      return id;
    },
  });

  // Wire up WebSocket
  useWebSocket();

  // Poll presence as a fallback (WS events are primary)
  usePresencePolling();

  // Fetch conversations (needed for DM peer_did lookup)
  const { data: conversations = [] } = useQuery({
    queryKey: ["conversations"],
    queryFn: () => conversationsApi.list(),
  });

  // Derive the peer DID for DM conversations
  const activePeerDid =
    activeConversation?.type === "dm"
      ? (conversations.find((c) => c.peer_did === activeConversation.peerId)?.peer_did ??
        activeConversation.peerId)
      : null;

  return (
    <div className="flex h-screen bg-surface-100 dark:bg-surface-950 overscroll-none select-none">
      <ConversationList />
      <main className="flex-1 overflow-hidden">
        {activeConversation?.type === "group" ? (
          <GroupView key={activeConversation.groupId} groupId={activeConversation.groupId} />
        ) : activePeerDid ? (
          <MessageView key={activePeerDid} peerDid={activePeerDid} />
        ) : (
          <div className="flex h-full items-center justify-center">
            <p className="text-sm text-surface-400">Select a conversation or start a new one</p>
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
  // Poll for node readiness when starting
  useNodeReady();

  // On app launch: if onboarded, auto-start the node (or prompt for passphrase)
  useEffect(() => {
    if (!isOnboarded) return;
    if (nodeStatus !== "idle") return;

    const startNode = async () => {
      try {
        // Always ask the backend for the identity path so VARIANCE_DATA_DIR is respected
        // when running a second instance with a different data directory.
        const path = await invoke<string>("default_identity_path");

        // If the identity file is gone (e.g. user deleted the data directory),
        // reset to onboarding rather than showing an opaque error.
        const exists = await invoke<boolean>("has_identity", { identityPath: path });
        if (!exists) {
          useIdentityStore.getState().reset();
          setNodeStatus("idle");
          return;
        }

        // If the identity file is encrypted, show the unlock screen instead of
        // trying to start without a passphrase (which would always fail).
        const encrypted = await invoke<boolean>("check_identity_encrypted", {
          identityPath: path,
        });
        if (encrypted) {
          setNodeStatus("needs-unlock");
          return;
        }

        setNodeStatus("starting");
        const port = await invoke<number>("start_node", { identityPath: path });
        setApiPort(port);
        resetApiBase();
        setNodeStatus("running");
      } catch (e) {
        // Swallow the "already starting" race from React StrictMode's double-mount.
        if (typeof e === "string" && e.includes("already starting")) return;
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

  if (nodeStatus === "needs-unlock") {
    return <UnlockScreen />;
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

import { useEffect, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useAppStore } from "../stores/appStore";
import { useIdentityStore } from "../stores/identityStore";
import { useMessagingStore } from "../stores/messagingStore";
import type { ActiveConversation } from "../stores/messagingStore";
import { variantWs } from "../api/websocket";
import type { WsEvent } from "../api/types";

// Per-conversation debounce: a burst of messages collapses into one notification.
// Each conversation gets a stable numeric id so the OS replaces (not stacks)
// the previous notification for that conversation.
// NOTIFY_TARGETS lets the action handler navigate to the right conversation on click.
const NOTIFY_TIMERS = new Map<string, ReturnType<typeof setTimeout>>();
const NOTIFY_BODIES = new Map<string, string>();
const NOTIFY_IDS = new Map<string, number>();
// Most-recent notification target + timestamp. When the window gains focus
// (which happens when the user clicks a notification banner on macOS/Windows),
// we navigate to this conversation if it was set recently enough.
let pendingNavTarget: ActiveConversation | null = null;
let pendingNavKey: string | null = null;
let pendingNavAt = 0;
let notifyIdCounter = 1;

function getNotifyId(conversationKey: string): number {
  if (!NOTIFY_IDS.has(conversationKey)) {
    NOTIFY_IDS.set(conversationKey, notifyIdCounter++);
  }
  return NOTIFY_IDS.get(conversationKey)!;
}

async function notify(conversationKey: string, body: string, target: ActiveConversation | null) {
  NOTIFY_BODIES.set(conversationKey, body);

  const existing = NOTIFY_TIMERS.get(conversationKey);
  if (existing) clearTimeout(existing);

  NOTIFY_TIMERS.set(
    conversationKey,
    setTimeout(async () => {
      NOTIFY_TIMERS.delete(conversationKey);
      const latestBody = NOTIFY_BODIES.get(conversationKey) ?? body;
      NOTIFY_BODIES.delete(conversationKey);

      let granted = await isPermissionGranted();
      if (!granted) {
        const permission = await requestPermission();
        granted = permission === "granted";
      }
      if (granted) {
        // Store pending navigation before firing — when the user clicks the banner,
        // macOS brings the window to front which triggers onFocusChanged.
        pendingNavTarget = target;
        pendingNavKey = conversationKey;
        pendingNavAt = Date.now();
        sendNotification({
          title: "Variance",
          body: latestBody,
          id: getNotifyId(conversationKey),
        });
      }
    }, 500)
  );
}

export function useWebSocket() {
  const nodeStatus = useAppStore((s) => s.nodeStatus);
  const localDid = useIdentityStore((s) => s.did);
  // Keep localDid in a ref so event handlers always read the latest value
  // without requiring the effect to re-run (and reconnect the WebSocket).
  const localDidRef = useRef(localDid);
  useEffect(() => {
    localDidRef.current = localDid;
  }, [localDid]);

  const queryClient = useQueryClient();
  const setPresence = useMessagingStore((s) => s.setPresence);
  const setPeerName = useMessagingStore((s) => s.setPeerName);
  const markUnread = useMessagingStore((s) => s.markUnread);
  const markRead = useMessagingStore((s) => s.markRead);
  const setActiveConversation = useMessagingStore((s) => s.setActiveConversation);
  const activeConversation = useMessagingStore((s) => s.activeConversation);
  const setTyping = useMessagingStore((s) => s.setTyping);

  useEffect(() => {
    if (nodeStatus !== "running") return;

    // When the user clicks a notification banner, macOS/Windows brings the window
    // to front. We intercept onFocusChanged to detect this and navigate to the
    // conversation that triggered the notification.
    const focusListenerPromise = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) return;
      const NAV_WINDOW_MS = 5_000;
      if (pendingNavTarget && Date.now() - pendingNavAt < NAV_WINDOW_MS) {
        setActiveConversation(pendingNavTarget);
        if (pendingNavKey) markRead(pendingNavKey);
        pendingNavTarget = null;
        pendingNavKey = null;
        pendingNavAt = 0;
      }
    });

    console.log("[WebSocket] Connecting...");
    void variantWs.connect();

    const off = variantWs.on((event: WsEvent) => {
      console.log("[WebSocket] Received event:", event.type, event);

      switch (event.type) {
        case "DirectMessageReceived": {
          console.log("[WebSocket] Processing DirectMessageReceived:", event.message_id);
          // They sent a message, so they're no longer typing.
          setTyping(event.from, event.from, false);
          // Invalidate this conversation's messages — MessageView will refetch.
          void queryClient.invalidateQueries({ queryKey: ["messages", event.from] });
          // Update the conversation list (timestamp, ordering).
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          // Mark conversation as unread + notify if it's not the active one
          const currentDid = localDidRef.current;
          if (currentDid) {
            const dids = [currentDid, event.from].sort();
            const conversationId = `${dids[0]}:${dids[1]}`;
            const isActive =
              activeConversation?.type === "dm" && activeConversation.peerId === event.from;
            if (!isActive) {
              markUnread(conversationId);
              const senderName =
                useMessagingStore.getState().peerNames.get(event.from) ?? "Someone";
              void notify(conversationId, `New message from ${senderName}`, {
                type: "dm",
                peerId: event.from,
              });
            }
          }
          break;
        }

        case "DirectMessageSent": {
          console.log("[WebSocket] Processing DirectMessageSent:", event.message_id);
          // onSettled in MessageInput already invalidates ["messages", peerDid].
          // We only need to refresh the conversation list here.
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          break;
        }

        case "GroupMessageReceived": {
          // Sender sent a message — clear their typing indicator.
          setTyping(event.from, `group:${event.group_id}`, false);
          void queryClient.invalidateQueries({ queryKey: ["messages", "group", event.group_id] });
          void queryClient.invalidateQueries({ queryKey: ["groups"] });
          const isActiveGroup =
            activeConversation?.type === "group" && activeConversation.groupId === event.group_id;
          if (!isActiveGroup) {
            markUnread(event.group_id);
            const senderName = useMessagingStore.getState().peerNames.get(event.from) ?? "Someone";
            void notify(event.group_id, `New group message from ${senderName}`, {
              type: "group",
              groupId: event.group_id,
            });
          }
          break;
        }

        case "MlsGroupJoined": {
          console.log(
            "[WebSocket] Auto-joined MLS group",
            event.group_id,
            "via invite from",
            event.inviter
          );
          void queryClient.invalidateQueries({ queryKey: ["groups"] });
          break;
        }

        case "ReceiptDelivered":
        case "ReceiptRead":
          void queryClient.invalidateQueries({ queryKey: ["receipts"] });
          break;

        case "TypingStarted":
          // DM: key by sender DID (the UI looks up typingUsers.get(peerDid))
          // Group: key by the group recipient ("group:{id}") so GroupView
          //        can look up typingUsers.get(`group:${groupId}`)
          if (event.recipient.startsWith("group:")) {
            setTyping(event.from, event.recipient, true);
          } else {
            setTyping(event.from, event.from, true);
          }
          break;

        case "TypingStopped":
          if (event.recipient.startsWith("group:")) {
            setTyping(event.from, event.recipient, false);
          } else {
            setTyping(event.from, event.from, false);
          }
          break;

        case "PresenceUpdated":
          console.log(
            `[WebSocket] Presence update: ${event.did} is ${event.online ? "online" : "offline"}`,
            event.display_name ? `(${event.display_name})` : "(no username)"
          );
          setPresence(event.did, event.online);
          if (event.display_name) {
            setPeerName(event.did, event.display_name);
          }
          // Refresh conversation list so peer_username from backend is up to date
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          break;

        case "OfflineMessagesReceived":
          // Relay delivered offline messages — refetch all message queries so the
          // user sees them immediately if they have a chat open.
          void queryClient.invalidateQueries({ queryKey: ["messages"] });
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          void notify("offline-relay", "You have new messages while you were away", null);
          break;

        case "PeerRenamed":
          // A connected peer changed their username — update display name and
          // refresh conversations so the new name appears immediately.
          setPeerName(event.did, event.display_name);
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          break;

        case "DirectMessageStatusChanged":
          // A message's delivery status changed (e.g. OutboundFailure after send)
          // Refetch messages so the UI updates the status icon (✓✓ → ⏰).
          // We don't know the peer DID from this event, so invalidate all message queries.
          void queryClient.invalidateQueries({ queryKey: ["messages"] });
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          break;

        default:
          break;
      }
    });

    return () => {
      off();
      variantWs.disconnect();
      void focusListenerPromise.then((unlisten) => unlisten());
    };
  }, [
    nodeStatus,
    queryClient,
    setPresence,
    setPeerName,
    markUnread,
    markRead,
    setActiveConversation,
    activeConversation,
    setTyping,
  ]);
}

import { useEffect, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
  removeActive,
  onAction,
  registerActionTypes,
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
const NOTIFY_TIMERS = new Map<string, ReturnType<typeof setTimeout>>();
const NOTIFY_BODIES = new Map<string, string>();
const NOTIFY_IDS = new Map<string, number>();
// Auto-clear timers: remove notifications from the OS notification center after 60s.
const NOTIFY_CLEAR_TIMERS = new Map<string, ReturnType<typeof setTimeout>>();
// Map notification ID → { target, conversationKey } so onAction can navigate.
const NOTIFY_ACTION_TARGETS = new Map<number, { target: ActiveConversation | null; key: string }>();
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

/** Remove a conversation's notification from the OS notification center. */
function clearConversationNotification(conversationKey: string) {
  const id = NOTIFY_IDS.get(conversationKey);
  if (id !== undefined) {
    void removeActive([{ id }]);
  }
  const timer = NOTIFY_CLEAR_TIMERS.get(conversationKey);
  if (timer) {
    clearTimeout(timer);
    NOTIFY_CLEAR_TIMERS.delete(conversationKey);
  }
  if (pendingNavKey === conversationKey) {
    pendingNavTarget = null;
    pendingNavKey = null;
    pendingNavAt = 0;
  }
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
      if (!granted) return;

      const id = getNotifyId(conversationKey);
      NOTIFY_ACTION_TARGETS.set(id, { target, key: conversationKey });

      // Always record the pending nav target so that focus-based navigation works
      // for both banner clicks (fast) and notification-center clicks (slow).
      // clearConversationNotification() will reset this when the user views the
      // conversation through any other path.
      if (target) {
        pendingNavTarget = target;
        pendingNavKey = conversationKey;
        pendingNavAt = Date.now();
      }

      sendNotification({
        title: "Variance",
        body: latestBody,
        id,
        actionTypeId: "message",
      });

      // Auto-clear from OS notification center after 60 seconds.
      const clearTimer = NOTIFY_CLEAR_TIMERS.get(conversationKey);
      if (clearTimer) clearTimeout(clearTimer);
      NOTIFY_CLEAR_TIMERS.set(
        conversationKey,
        setTimeout(() => {
          void removeActive([{ id }]);
          NOTIFY_CLEAR_TIMERS.delete(conversationKey);
        }, 60_000)
      );
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

  // Clear the conversation's OS notification when the user opens it.
  useEffect(() => {
    if (!activeConversation || !localDid) return;
    const key =
      activeConversation.type === "dm"
        ? [...[localDid, activeConversation.peerId]].sort().join(":")
        : activeConversation.groupId;
    clearConversationNotification(key);
  }, [activeConversation, localDid]);

  useEffect(() => {
    if (nodeStatus !== "running") return;

    // Register the "View" action button so users can explicitly navigate from
    // a notification without having to click the banner body (which on macOS
    // only brings the window to front — it doesn't tell us which conversation).
    void registerActionTypes([
      { id: "message", actions: [{ id: "view", title: "View", foreground: true }] },
    ]);

    // onAction fires when the user clicks the explicit "View" action button.
    // Navigate immediately and clear the notification and pending focus-nav state.
    const actionListenerPromise = onAction((notification) => {
      if (notification.id === undefined) return;
      const info = NOTIFY_ACTION_TARGETS.get(notification.id);
      if (!info) return;
      const { target, key } = info;
      if (target) {
        setActiveConversation(target);
        markRead(key);
      }
      clearConversationNotification(key);
    });

    // When the user clicks a notification banner body (macOS/Windows), the OS
    // brings the window to front — intercept onFocusChanged and navigate.
    // We use a 30 s window to cover both fast banner clicks (~200 ms) and slower
    // notification-center clicks. clearConversationNotification() resets
    // pendingNavTarget when the conversation is opened via any other path, which
    // prevents stale notifications from hijacking navigation.
    const focusListenerPromise = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) return;
      const NAV_WINDOW_MS = 30_000;
      if (pendingNavTarget && Date.now() - pendingNavAt < NAV_WINDOW_MS) {
        setActiveConversation(pendingNavTarget);
        if (pendingNavKey) {
          markRead(pendingNavKey);
          clearConversationNotification(pendingNavKey);
        }
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
      void actionListenerPromise.then((listener) => listener.unregister());
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

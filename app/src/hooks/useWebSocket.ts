import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useAppStore } from "../stores/appStore";
import { useIdentityStore } from "../stores/identityStore";
import { useMessagingStore } from "../stores/messagingStore";
import { variantWs } from "../api/websocket";
import type { WsEvent } from "../api/types";

/**
 * Connect the WebSocket when the node is running and dispatch incoming events.
 *
 * For DirectMessageReceived we bump a Zustand tick instead of calling
 * invalidateQueries with event.from. The tick is watched by MessageView, which
 * calls its own refetch() — avoiding any dependency on whether event.from
 * exactly matches the React Query key used by the mounted component.
 */
export function useWebSocket() {
  const nodeStatus = useAppStore((s) => s.nodeStatus);
  const localDid = useIdentityStore((s) => s.did);
  const queryClient = useQueryClient();
  const tickInboundMessage = useMessagingStore((s) => s.tickInboundMessage);
  const setPresence = useMessagingStore((s) => s.setPresence);
  const setPeerName = useMessagingStore((s) => s.setPeerName);
  const markUnread = useMessagingStore((s) => s.markUnread);
  const activeConversationId = useMessagingStore((s) => s.activeConversationId);

  useEffect(() => {
    if (nodeStatus !== "running") return;

    console.log("[WebSocket] Connecting...");
    void variantWs.connect();

    const off = variantWs.on((event: WsEvent) => {
      console.log("[WebSocket] Received event:", event.type, event);

      switch (event.type) {
        case "DirectMessageReceived": {
          console.log("[WebSocket] Processing DirectMessageReceived:", event.message_id);
          // Bump the tick — MessageView will call refetch() in response.
          tickInboundMessage();
          // Update the conversation list (timestamp, ordering).
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          // Mark conversation as unread if it's not the active one
          // Generate conversation ID matching backend: sorted DIDs joined with ":"
          if (localDid) {
            const dids = [localDid, event.from].sort();
            const conversationId = `${dids[0]}:${dids[1]}`;
            if (conversationId !== activeConversationId) {
              markUnread(conversationId);
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

        case "GroupMessageReceived":
          void queryClient.invalidateQueries({
            queryKey: ["messages", "group", event.group_id],
          });
          break;

        case "ReceiptDelivered":
        case "ReceiptRead":
          void queryClient.invalidateQueries({ queryKey: ["receipts"] });
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

        default:
          break;
      }
    });

    return () => {
      off();
      variantWs.disconnect();
    };
  }, [
    nodeStatus,
    queryClient,
    tickInboundMessage,
    setPresence,
    setPeerName,
    markUnread,
    activeConversationId,
    localDid,
  ]);
}

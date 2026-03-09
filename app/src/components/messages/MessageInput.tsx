import { useMutation, useQueryClient } from "@tanstack/react-query";
import { messagesApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { MessageEditor } from "./MessageEditor";
import type { DirectMessage } from "../../api/types";

interface MessageInputProps {
  peerDid: string;
}

export function MessageInput({ peerDid }: MessageInputProps) {
  const queryClient = useQueryClient();
  const localDid = useIdentityStore((s) => s.did);

  const sendMutation = useMutation({
    mutationFn: (message: string) =>
      messagesApi.sendDirect({ recipient_did: peerDid, text: message }),
    onMutate: async (messageText) => {
      await queryClient.cancelQueries({ queryKey: ["messages", peerDid] });
      const previousMessages = queryClient.getQueryData<DirectMessage[]>(["messages", peerDid]);

      if (localDid) {
        const optimisticMessage: DirectMessage = {
          id: `temp-${Date.now()}`,
          sender_did: localDid,
          recipient_did: peerDid,
          text: messageText,
          timestamp: Date.now(),
          status: "pending",
        };
        queryClient.setQueryData<DirectMessage[]>(["messages", peerDid], (old = []) => [
          ...old,
          optimisticMessage,
        ]);
      }

      return { previousMessages };
    },
    onError: (_err, _message, context) => {
      if (context?.previousMessages) {
        queryClient.setQueryData(["messages", peerDid], context.previousMessages);
      }
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["messages", peerDid] });
      void queryClient.invalidateQueries({ queryKey: ["conversations"] });
    },
  });

  return (
    <MessageEditor
      placeholder="Message"
      onSend={(md) => sendMutation.mutate(md)}
      isPending={sendMutation.isPending}
      typing={{ recipient: peerDid, isGroup: false, cooldownMs: 3_000, stopDelayMs: 2_000 }}
    />
  );
}

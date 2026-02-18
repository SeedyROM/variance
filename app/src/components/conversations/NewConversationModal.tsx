import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "../ui/Dialog";
import { Input } from "../ui/Input";
import { Button } from "../ui/Button";
import { conversationsApi } from "../../api/client";

interface NewConversationModalProps {
  open: boolean;
  onClose: () => void;
  onCreated: (conversationId: string) => void;
}

export function NewConversationModal({ open, onClose, onCreated }: NewConversationModalProps) {
  const [recipientDid, setRecipientDid] = useState("");
  const [initialText, setInitialText] = useState("Hello!");
  const queryClient = useQueryClient();

  const mutation = useMutation({
    mutationFn: () =>
      conversationsApi.start({
        recipient_did: recipientDid.trim(),
        text: initialText,
      }),
    onSuccess: (data) => {
      void queryClient.invalidateQueries({ queryKey: ["conversations"] });
      onCreated(data.conversation_id);
      setRecipientDid("");
      setInitialText("Hello!");
      onClose();
    },
  });

  const isValid = recipientDid.trim().startsWith("did:") && initialText.trim().length > 0;

  return (
    <Dialog open={open} onClose={onClose} title="New Conversation">
      <div className="flex flex-col gap-4">
        <Input
          label="Recipient DID"
          value={recipientDid}
          onChange={(e) => setRecipientDid(e.target.value)}
          placeholder="did:variance:..."
          error={
            recipientDid && !recipientDid.startsWith("did:") ? "Must start with did:" : undefined
          }
        />

        <Input
          label="First message"
          value={initialText}
          onChange={(e) => setInitialText(e.target.value)}
          placeholder="Hello!"
        />

        {mutation.error && <p className="text-xs text-red-500">{String(mutation.error)}</p>}

        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={!isValid}
            loading={mutation.isPending}
            onClick={() => mutation.mutate()}
          >
            Start conversation
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

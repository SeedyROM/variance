import { useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Send } from "lucide-react";
import { messagesApi, typingApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import { useSettingsStore } from "../../stores/settingsStore";
import { cn } from "../../utils/cn";
import type { DirectMessage } from "../../api/types";

/** Don't send another /typing/start within this window (ms). */
const TYPING_SEND_COOLDOWN_MS = 3_000;

interface MessageInputProps {
  peerDid: string;
}

export function MessageInput({ peerDid }: MessageInputProps) {
  const [text, setText] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const typingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Timestamp of the last *actual* typing-start request we fired. */
  const lastTypingSentRef = useRef<number>(0);
  const queryClient = useQueryClient();
  const localDid = useIdentityStore((s) => s.did);
  const tabSize = useSettingsStore((s) => s.tabSize);

  const sendMutation = useMutation({
    mutationFn: (message: string) =>
      messagesApi.sendDirect({ recipient_did: peerDid, text: message }),
    onMutate: async (messageText) => {
      // Cancel any outgoing refetches
      await queryClient.cancelQueries({ queryKey: ["messages", peerDid] });

      // Snapshot the previous value
      const previousMessages = queryClient.getQueryData<DirectMessage[]>(["messages", peerDid]);

      // Optimistically update to show the new message immediately
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
      // Rollback on error
      if (context?.previousMessages) {
        queryClient.setQueryData(["messages", peerDid], context.previousMessages);
      }
    },
    onSettled: () => {
      // Refetch to sync with backend state
      // TODO: Rely purely on WebSocket events once they're working reliably
      void queryClient.invalidateQueries({ queryKey: ["messages", peerDid] });
      void queryClient.invalidateQueries({ queryKey: ["conversations"] });
    },
  });

  const handleSend = () => {
    const trimmed = text.trim();
    if (!trimmed || sendMutation.isPending) return;

    setText("");
    if (textareaRef.current) {
      textareaRef.current.style.height = "auto";
    }

    // Cancel any pending typing-stop timer and immediately signal stop
    if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
    lastTypingSentRef.current = 0; // allow immediate re-send on next input
    void typingApi.stop({ recipient: peerDid, is_group: false });

    sendMutation.mutate(trimmed);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Tab") {
      e.preventDefault();
      const el = e.currentTarget;
      const { selectionStart, selectionEnd, value } = el;
      const spaces = " ".repeat(tabSize);
      const next = value.slice(0, selectionStart) + spaces + value.slice(selectionEnd);
      setText(next);
      requestAnimationFrame(() => {
        el.selectionStart = el.selectionEnd = selectionStart + tabSize;
      });
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setText(e.target.value);

    // Auto-resize
    e.target.style.height = "auto";
    e.target.style.height = `${e.target.scrollHeight}px`;

    // Typing indicator — only fire the HTTP request if we haven't sent one
    // recently. The server also enforces its own cooldown, but skipping here
    // avoids unnecessary HTTP round-trips on every keystroke.
    const now = Date.now();
    if (now - lastTypingSentRef.current >= TYPING_SEND_COOLDOWN_MS) {
      lastTypingSentRef.current = now;
      void typingApi.start({ recipient: peerDid, is_group: false });
    }

    // Reset the stop-typing timer on every keystroke — if the user pauses for
    // 2s we send a stop, which also clears the server cooldown so the next
    // keystroke will send a fresh start.
    if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
    typingTimerRef.current = setTimeout(() => {
      lastTypingSentRef.current = 0; // allow immediate re-send after stop
      void typingApi.stop({ recipient: peerDid, is_group: false });
    }, 2000);
  };

  return (
    <div className="border-t border-surface-200 bg-surface-50 px-4 py-3 dark:border-surface-800 dark:bg-surface-900">
      <div className="flex items-center gap-2 rounded-xl border border-surface-300 bg-white px-3 py-2 focus-within:border-primary-500 focus-within:ring-2 focus-within:ring-primary-500/20 dark:border-surface-700 dark:bg-surface-950">
        <textarea
          ref={textareaRef}
          value={text}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder="Message"
          rows={1}
          className={cn(
            "max-h-40 flex-1 resize-none bg-transparent text-sm text-surface-900 placeholder:text-surface-400 focus:outline-none dark:text-surface-50"
          )}
        />
        <button
          onClick={handleSend}
          disabled={!text.trim() || sendMutation.isPending}
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-primary-500 text-white transition-colors hover:bg-primary-600 disabled:opacity-40"
        >
          <Send className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

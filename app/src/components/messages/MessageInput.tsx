import { useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Send } from "lucide-react";
import { messagesApi, typingApi } from "../../api/client";
import { cn } from "../../utils/cn";

interface MessageInputProps {
  peerDid: string;
}

export function MessageInput({ peerDid }: MessageInputProps) {
  const [text, setText] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const typingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const queryClient = useQueryClient();

  const sendMutation = useMutation({
    mutationFn: (message: string) =>
      messagesApi.sendDirect({ recipient_did: peerDid, text: message }),
    onSuccess: () => {
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
    sendMutation.mutate(trimmed);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
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

    // Typing indicator with debounce
    void typingApi.start({ recipient: peerDid, is_group: false });
    if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
    typingTimerRef.current = setTimeout(() => {
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

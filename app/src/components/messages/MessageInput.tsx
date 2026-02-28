import { useRef } from "react";
import { useEditor, EditorContent } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import Placeholder from "@tiptap/extension-placeholder";
import { Markdown } from "tiptap-markdown";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Send } from "lucide-react";
import { messagesApi, typingApi } from "../../api/client";
import { useIdentityStore } from "../../stores/identityStore";
import type { DirectMessage } from "../../api/types";

/** Don't send another /typing/start within this window (ms). */
const TYPING_SEND_COOLDOWN_MS = 3_000;

interface MessageInputProps {
  peerDid: string;
}

export function MessageInput({ peerDid }: MessageInputProps) {
  const typingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastTypingSentRef = useRef<number>(0);
  // Stable ref so handleKeyDown can always call the latest send without stale closures
  const sendRef = useRef<() => void>(() => {});
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

  const editor = useEditor({
    extensions: [StarterKit, Markdown, Placeholder.configure({ placeholder: "Message" })],
    content: "",
    editorProps: {
      attributes: {
        class:
          "max-h-40 overflow-y-auto text-sm text-surface-900 dark:text-surface-50 focus:outline-none prose prose-sm dark:prose-invert max-w-none",
      },
      handleKeyDown(_view, event) {
        if (event.key === "Enter" && !event.shiftKey) {
          event.preventDefault();
          sendRef.current();
          return true;
        }
        return false;
      },
    },
    onUpdate({ editor }) {
      const md = editor.storage.markdown.getMarkdown() as string;
      if (!md.trim()) return;
      const now = Date.now();
      if (now - lastTypingSentRef.current >= TYPING_SEND_COOLDOWN_MS) {
        lastTypingSentRef.current = now;
        void typingApi.start({ recipient: peerDid, is_group: false });
      }
      if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
      typingTimerRef.current = setTimeout(() => {
        lastTypingSentRef.current = 0;
        void typingApi.stop({ recipient: peerDid, is_group: false });
      }, 2000);
    },
  });

  // Keep sendRef up to date every render so handleKeyDown always calls the latest version
  sendRef.current = () => {
    if (!editor || sendMutation.isPending) return;
    const md = (editor.storage.markdown.getMarkdown() as string).trim();
    if (!md) return;

    editor.commands.clearContent();

    if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
    lastTypingSentRef.current = 0;
    void typingApi.stop({ recipient: peerDid, is_group: false });

    sendMutation.mutate(md);
  };

  const isEmpty = !editor || editor.isEmpty;

  return (
    <div className="border-t border-surface-200 bg-surface-50 px-4 py-3 dark:border-surface-800 dark:bg-surface-900">
      <div className="flex items-center gap-2 rounded-xl border border-surface-300 bg-white px-3 py-2 focus-within:border-primary-500 focus-within:ring-2 focus-within:ring-primary-500/20 dark:border-surface-700 dark:bg-surface-950">
        <EditorContent editor={editor} className="flex-1 min-w-0" />
        <button
          onClick={() => sendRef.current()}
          disabled={isEmpty || sendMutation.isPending}
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-primary-500 text-white transition-colors hover:bg-primary-600 disabled:opacity-40"
        >
          <Send className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

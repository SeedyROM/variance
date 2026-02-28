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
    extensions: [
      StarterKit.configure({
        hardBreak: false, // We handle Shift+Enter ourselves in handleKeyDown
        heading: { levels: [1, 2, 3] },
      }),
      Markdown.configure({ breaks: true }),
      Placeholder.configure({ placeholder: "Message" }),
    ],
    content: "",
    editorProps: {
      attributes: {
        class:
          "max-h-40 overflow-y-auto text-sm text-surface-900 dark:text-surface-50 focus:outline-none prose prose-sm dark:prose-invert max-w-none",
      },
      handleKeyDown(view, event) {
        if (event.key === "Enter" && !event.shiftKey) {
          event.preventDefault();
          sendRef.current();
          return true;
        }
        // Shift+Enter: new line, but clear block-level formatting (Slack-style).
        // Handled here because ProseMirror's handleKeyDown fires before plugin
        // keymaps, so we must intercept before HardBreak inserts a <br>.
        if (event.key === "Enter" && event.shiftKey) {
          const { state, dispatch } = view;
          const { $from } = state.selection;
          const parentType = $from.parent.type.name;

          if (parentType === "heading" || parentType === "blockquote") {
            // Split to new block, then convert it to a paragraph
            const tr = state.tr.split(state.selection.from);
            const pos = tr.selection.from;
            tr.setNodeMarkup(
              tr.doc.resolve(pos).before(tr.doc.resolve(pos).depth),
              state.schema.nodes.paragraph
            );
            dispatch(tr);
          } else {
            // Default: split block (new paragraph, not a <br>)
            const tr = state.tr.split(state.selection.from);
            dispatch(tr);
          }
          return true;
        }
        // Escape: clear all inline marks (bold, italic, etc.) at cursor.
        // Matches Slack/Discord behaviour — quick way to "reset" formatting
        // when the cursor gets stuck inside a mark after editing.
        if (event.key === "Escape") {
          const { state, dispatch } = view;
          const { $from } = state.selection;
          // Only act when there are active marks at cursor
          const marks = $from.marks();
          if (marks.length > 0) {
            dispatch(state.tr.setStoredMarks([]));
            return true;
          }
          return false;
        }
        // Prevent Tab from stealing focus; it has no useful meaning in chat input
        if (event.key === "Tab") {
          event.preventDefault();
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

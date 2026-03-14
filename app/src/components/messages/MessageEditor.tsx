import { useEffect, useRef, useState } from "react";
import { useEditor, EditorContent } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import Placeholder from "@tiptap/extension-placeholder";
import { Markdown } from "tiptap-markdown";
import { typingApi } from "../../api/client";
import { MessageComposerShell, MAX_MESSAGE_LENGTH } from "./MessageComposerShell";

interface TypingConfig {
  recipient: string;
  isGroup: boolean;
  /** Minimum interval between outbound typing-start signals (ms). Default 3000. */
  cooldownMs?: number;
  /** Delay before sending a typing-stop after the last keystroke (ms). Default 2000. */
  stopDelayMs?: number;
}

interface MessageEditorProps {
  placeholder: string;
  /** Called with the trimmed markdown string when the user sends. */
  onSend: (markdown: string) => void;
  /** Disables the send button (e.g. while a mutation is in-flight). */
  isPending: boolean;
  typing: TypingConfig;
}

/**
 * TipTap-backed markdown message editor with typing indicator management.
 *
 * Handles the editor lifecycle, keyboard shortcuts (Enter to send,
 * Shift+Enter for new line, Escape to clear marks), character counting,
 * and outbound typing signals. Callers only need to provide a send callback
 * and typing config.
 */
export function MessageEditor({ placeholder, onSend, isPending, typing }: MessageEditorProps) {
  const cooldownMs = typing.cooldownMs ?? 3_000;
  const stopDelayMs = typing.stopDelayMs ?? 2_000;

  const typingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastTypingSentRef = useRef<number>(0);
  const sendRef = useRef<() => void>(() => {});
  const [charCount, setCharCount] = useState(0);

  // Cancel any pending stop-typing timer on unmount to avoid firing stale
  // events for the old conversation after a switch.
  useEffect(
    () => () => {
      if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
    },
    []
  );

  const editor = useEditor({
    extensions: [
      StarterKit.configure({
        hardBreak: false,
        heading: { levels: [1, 2, 3] },
      }),
      Markdown.configure({ breaks: true }),
      Placeholder.configure({ placeholder }),
    ],
    content: "",
    editorProps: {
      attributes: {
        class:
          "max-h-40 overflow-y-auto text-sm text-surface-900 dark:text-surface-50 focus:outline-none prose prose-sm dark:prose-invert max-w-none",
        spellcheck: "true",
        autocorrect: "on",
        autocapitalize: "sentences",
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
            const tr = state.tr.split(state.selection.from);
            const pos = tr.selection.from;
            tr.setNodeMarkup(
              tr.doc.resolve(pos).before(tr.doc.resolve(pos).depth),
              state.schema.nodes.paragraph
            );
            dispatch(tr);
          } else {
            const tr = state.tr.split(state.selection.from);
            dispatch(tr);
          }
          return true;
        }
        // Escape: clear all inline marks (bold, italic, etc.) at cursor.
        if (event.key === "Escape") {
          const { state, dispatch } = view;
          const { $from } = state.selection;
          const marks = $from.marks();
          if (marks.length > 0) {
            dispatch(state.tr.setStoredMarks([]));
            return true;
          }
          return false;
        }
        // Prevent Tab from stealing focus
        if (event.key === "Tab") {
          event.preventDefault();
          return true;
        }
        return false;
      },
    },
    onUpdate({ editor: ed }) {
      const md = ed.storage.markdown.getMarkdown() as string;
      setCharCount(md.length);
      if (!md.trim()) return;
      const now = Date.now();
      if (now - lastTypingSentRef.current >= cooldownMs) {
        lastTypingSentRef.current = now;
        void typingApi.start({ recipient: typing.recipient, is_group: typing.isGroup });
      }
      if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
      typingTimerRef.current = setTimeout(() => {
        lastTypingSentRef.current = 0;
        void typingApi.stop({ recipient: typing.recipient, is_group: typing.isGroup });
      }, stopDelayMs);
    },
  });

  const isOverLimit = charCount > MAX_MESSAGE_LENGTH;

  // Keep sendRef up to date every render so handleKeyDown always calls the latest version
  sendRef.current = () => {
    if (!editor || isPending || isOverLimit) return;
    const md = (editor.storage.markdown.getMarkdown() as string).trim();
    if (!md) return;

    editor.commands.clearContent();

    if (typingTimerRef.current) clearTimeout(typingTimerRef.current);
    lastTypingSentRef.current = 0;
    void typingApi.stop({ recipient: typing.recipient, is_group: typing.isGroup });

    onSend(md);
  };

  return (
    <MessageComposerShell
      charCount={charCount}
      isEmpty={!editor || editor.isEmpty}
      isPending={isPending}
      onSend={() => sendRef.current()}
    >
      <EditorContent editor={editor} className="flex-1 min-w-0" />
    </MessageComposerShell>
  );
}

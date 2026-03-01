import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "../ui/Dialog";
import { Input } from "../ui/Input";
import { Button } from "../ui/Button";
import { conversationsApi, identityApi } from "../../api/client";
import type { ResolvedUsername, ResolvedUsernameMultiple } from "../../api/types";

interface NewConversationModalProps {
  open: boolean;
  onClose: () => void;
  onCreated: (conversationId: string) => void;
}

/** Returns true if the input looks like a DID (starts with "did:"). */
function isDid(input: string): boolean {
  return input.trimStart().startsWith("did:");
}

/** Returns true if the input looks like a username (possibly with #discriminator). */
function isUsernameLike(input: string): boolean {
  const trimmed = input.trim();
  if (!trimmed || isDid(trimmed)) return false;
  // Must start with a letter
  return /^[a-zA-Z]/.test(trimmed);
}

export function NewConversationModal({ open, onClose, onCreated }: NewConversationModalProps) {
  const [recipient, setRecipient] = useState("");
  const [initialText, setInitialText] = useState("Hello!");
  const [resolvedDid, setResolvedDid] = useState<string | null>(null);
  const [resolvedDisplay, setResolvedDisplay] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);
  const [resolveError, setResolveError] = useState<string | null>(null);
  const [multipleMatches, setMultipleMatches] = useState<ResolvedUsername[] | null>(null);
  const queryClient = useQueryClient();

  const resetResolution = () => {
    setResolvedDid(null);
    setResolvedDisplay(null);
    setResolveError(null);
    setMultipleMatches(null);
  };

  const handleRecipientChange = (value: string) => {
    setRecipient(value);
    resetResolution();
  };

  const resolveRecipient = async (): Promise<string | null> => {
    const trimmed = recipient.trim();

    // Direct DID — no resolution needed
    if (isDid(trimmed)) {
      return trimmed;
    }

    // Username resolution
    if (!isUsernameLike(trimmed)) return null;

    setResolving(true);
    setResolveError(null);
    try {
      const result = await identityApi.resolveUsername(trimmed);

      // Single match
      if ("did" in result && !("matches" in result)) {
        const single = result as ResolvedUsername;
        setResolvedDid(single.did);
        setResolvedDisplay(single.display_name);
        return single.did;
      }

      // Multiple matches
      const multi = result as ResolvedUsernameMultiple;
      if (multi.matches && multi.matches.length > 0) {
        setMultipleMatches(multi.matches);
        return null; // User must pick
      }

      setResolveError("No user found with that username");
      return null;
    } catch (e) {
      setResolveError(String(e));
      return null;
    } finally {
      setResolving(false);
    }
  };

  const selectMatch = (match: ResolvedUsername) => {
    setResolvedDid(match.did);
    setResolvedDisplay(match.display_name);
    setRecipient(match.display_name);
    setMultipleMatches(null);
  };

  const mutation = useMutation({
    mutationFn: async () => {
      const did = resolvedDid ?? (await resolveRecipient());
      if (!did) throw new Error("Could not resolve recipient");
      const data = await conversationsApi.start({
        recipient_did: did,
        text: initialText,
      });
      return { data, peerDid: did };
    },
    onSuccess: ({ peerDid }) => {
      void queryClient.invalidateQueries({ queryKey: ["conversations"] });
      onCreated(peerDid);
      setRecipient("");
      setInitialText("Hello!");
      resetResolution();
      onClose();
    },
  });

  const trimmed = recipient.trim();
  const inputIsValid =
    (isDid(trimmed) || isUsernameLike(trimmed) || resolvedDid !== null) &&
    initialText.trim().length > 0;

  const getInputHint = (): string | undefined => {
    if (!trimmed) return undefined;
    if (isDid(trimmed)) return undefined;
    if (resolvedDid && resolvedDisplay) return undefined;
    if (!isUsernameLike(trimmed) && !isDid(trimmed)) return "Enter a DID or username";
    return undefined;
  };

  return (
    <Dialog open={open} onClose={onClose} title="New Conversation">
      <div className="flex flex-col gap-4">
        <div>
          <Input
            label="Recipient"
            value={recipient}
            onChange={(e) => handleRecipientChange(e.target.value)}
            placeholder="username#0001 or did:variance:..."
            error={getInputHint()}
          />
          {resolvedDid && resolvedDisplay && (
            <p className="mt-1.5 text-xs text-green-600 dark:text-green-400">
              Resolved to <span className="font-semibold">{resolvedDisplay}</span>{" "}
              <span className="font-mono text-surface-500">({resolvedDid.slice(-12)})</span>
            </p>
          )}
          {resolving && <p className="mt-1.5 text-xs text-surface-500">Looking up username…</p>}
          {resolveError && <p className="mt-1.5 text-xs text-red-500">{resolveError}</p>}
        </div>

        {/* Multiple matches picker */}
        {multipleMatches && multipleMatches.length > 0 && (
          <div className="rounded-lg border border-surface-200 dark:border-surface-700 p-2">
            <p className="mb-2 text-xs font-medium text-surface-600 dark:text-surface-400">
              Multiple users found — pick one:
            </p>
            <div className="flex flex-col gap-1">
              {multipleMatches.map((match) => (
                <button
                  key={match.did}
                  onClick={() => selectMatch(match)}
                  className="flex items-center justify-between rounded-md px-3 py-2 text-left text-sm hover:bg-surface-100 dark:hover:bg-surface-800"
                >
                  <span className="font-medium text-surface-900 dark:text-surface-50">
                    {match.display_name}
                  </span>
                  <span className="font-mono text-xs text-surface-500">{match.did.slice(-12)}</span>
                </button>
              ))}
            </div>
          </div>
        )}

        <Input
          label="First message"
          value={initialText}
          onChange={(e) => setInitialText(e.target.value)}
          placeholder="Hello!"
          allowSuggestions
        />

        {mutation.error && <p className="text-xs text-red-500">{String(mutation.error)}</p>}

        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={!inputIsValid}
            loading={mutation.isPending || resolving}
            onClick={() => mutation.mutate()}
          >
            Start conversation
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

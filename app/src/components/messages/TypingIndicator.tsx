import { useMessagingStore } from "../../stores/messagingStore";

interface TypingIndicatorProps {
  users: string[];
}

/** Three animated dots used in both the message view and conversation list. */
export function TypingDots({ className = "" }: { className?: string }) {
  return (
    <span className={`inline-flex items-center gap-0.75 ${className}`}>
      {[0, 1, 2].map((i) => (
        <span
          key={i}
          className="h-1.25 w-1.25 rounded-full bg-current"
          style={{
            animation: "typing-dot 1.4s ease-in-out infinite",
            animationDelay: `${i * 160}ms`,
          }}
        />
      ))}
    </span>
  );
}

function shortName(name: string): string {
  // Strip discriminator (e.g. "alice#0042" → "alice")
  return name.split("#")[0];
}

/** Max users before we stop showing the indicator entirely. */
const MAX_TYPING_USERS = 8;
/** Max names listed explicitly before collapsing to "and N others". */
const MAX_NAMED_USERS = 3;

function formatTypingLabel(names: string[]): string {
  const short = names.map(shortName);
  if (short.length === 1) return `${short[0]} is typing`;
  if (short.length === 2) return `${short[0]} and ${short[1]} are typing`;
  if (short.length <= MAX_NAMED_USERS) {
    const last = short.pop()!;
    return `${short.join(", ")} and ${last} are typing`;
  }
  const shown = short.slice(0, MAX_NAMED_USERS);
  const remaining = short.length - MAX_NAMED_USERS;
  return `${shown.join(", ")} and ${remaining} ${remaining === 1 ? "other" : "others"} are typing`;
}

export function TypingIndicator({ users }: TypingIndicatorProps) {
  const peerNames = useMessagingStore((s) => s.peerNames);

  if (users.length === 0 || users.length > MAX_TYPING_USERS) return null;

  const displayNames = users.map((did) => peerNames.get(did) ?? did.slice(-8));

  return (
    <div className="flex items-center gap-2 px-4 py-1.5 text-surface-500">
      <TypingDots />
      <span className="text-xs">{formatTypingLabel(displayNames)}</span>
    </div>
  );
}

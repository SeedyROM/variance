import { useMessagingStore } from "../../stores/messagingStore";

interface TypingIndicatorProps {
  users: string[];
}

/** Three animated dots used in both the message view and conversation list. */
export function TypingDots({ className = "" }: { className?: string }) {
  return (
    <span className={`inline-flex items-center gap-[3px] ${className}`}>
      {[0, 1, 2].map((i) => (
        <span
          key={i}
          className="h-[5px] w-[5px] rounded-full bg-current"
          style={{
            animation: "typing-dot 1.4s ease-in-out infinite",
            animationDelay: `${i * 160}ms`,
          }}
        />
      ))}
    </span>
  );
}

function formatTypingLabel(names: string[]): string {
  if (names.length === 1) return `${names[0]} is typing`;
  if (names.length === 2) return `${names[0]} and ${names[1]} are typing`;
  return `${names[0]} and ${names.length - 1} others are typing`;
}

export function TypingIndicator({ users }: TypingIndicatorProps) {
  const peerNames = useMessagingStore((s) => s.peerNames);

  if (users.length === 0) return null;

  const displayNames = users.map((did) => peerNames.get(did) ?? did.slice(-8));

  return (
    <div className="flex items-center gap-2 px-4 py-1.5 text-surface-500">
      <TypingDots />
      <span className="text-xs">{formatTypingLabel(displayNames)}</span>
    </div>
  );
}

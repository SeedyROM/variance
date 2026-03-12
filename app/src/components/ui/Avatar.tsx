import { cn } from "../../utils/cn";

interface AvatarProps {
  did: string;
  /** Display name — first character is used as the avatar initial when provided. */
  name?: string;
  size?: "sm" | "md" | "lg";
  className?: string;
}

/** Deterministic color avatar derived from the DID string. */
export function Avatar({ did, name, size = "md", className }: AvatarProps) {
  // Use last 2 chars of the DID hex as a simple color seed
  const seed = did.slice(-2);
  const hue = (parseInt(seed, 16) / 255) * 360;
  const initial = name
    ? name.charAt(0).toUpperCase()
    : did.charAt(did.lastIndexOf(":") + 1).toUpperCase();

  return (
    <div
      className={cn(
        "flex shrink-0 items-center justify-center rounded-full font-semibold text-white select-none",
        {
          "h-7 w-7 text-xs": size === "sm",
          "h-9 w-9 text-sm": size === "md",
          "h-12 w-12 text-base": size === "lg",
        },
        className
      )}
      style={{ backgroundColor: `oklch(0.55 0.18 ${hue})` }}
    >
      {initial}
    </div>
  );
}

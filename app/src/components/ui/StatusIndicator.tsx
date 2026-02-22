import { cn } from "../../utils/cn";

type StatusIndicatorSize = "xs" | "sm" | "md";

interface StatusDotProps {
  online: boolean;
  size?: StatusIndicatorSize;
  /** Extra classes on the dot (e.g. border for avatar overlay) */
  className?: string;
}

const dotSizes: Record<StatusIndicatorSize, string> = {
  xs: "w-1.5 h-1.5",
  sm: "w-2 h-2",
  md: "w-3 h-3",
};

/** Colored dot indicating online/offline status. */
export function StatusDot({ online, size = "sm", className }: StatusDotProps) {
  return (
    <div
      className={cn(
        "shrink-0 rounded-full",
        dotSizes[size],
        online ? "bg-green-500" : "bg-surface-300 dark:bg-surface-500",
        className
      )}
    />
  );
}

interface StatusLabelProps {
  online: boolean;
  size?: StatusIndicatorSize;
  className?: string;
}

const labelSizes: Record<StatusIndicatorSize, string> = {
  xs: "text-[11px]",
  sm: "text-xs",
  md: "text-sm",
};

/** Text label showing "Online" or "Offline" with appropriate coloring. */
export function StatusLabel({ online, size = "sm", className }: StatusLabelProps) {
  return (
    <span
      className={cn(
        labelSizes[size],
        online ? "text-green-600 dark:text-green-400" : "text-surface-400 dark:text-surface-500",
        className
      )}
    >
      {online ? "Online" : "Offline"}
    </span>
  );
}

interface StatusIndicatorProps {
  online: boolean;
  size?: StatusIndicatorSize;
  className?: string;
}

/** Dot + label combo for inline status display. */
export function StatusIndicator({ online, size = "sm", className }: StatusIndicatorProps) {
  return (
    <div className={cn("flex items-center gap-1.5", className)}>
      <StatusDot online={online} size={size} />
      <StatusLabel online={online} size={size} />
    </div>
  );
}

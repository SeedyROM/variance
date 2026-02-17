import { cn } from "../../utils/cn";

interface BadgeProps {
  count: number;
  className?: string;
}

export function Badge({ count, className }: BadgeProps) {
  if (count === 0) return null;

  return (
    <span
      className={cn(
        "flex h-5 min-w-5 items-center justify-center rounded-full bg-primary-500 px-1 text-[11px] font-semibold text-white",
        className
      )}
    >
      {count > 99 ? "99+" : count}
    </span>
  );
}

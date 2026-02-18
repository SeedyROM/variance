import { dayLabel } from "../../utils/time";

interface DateDividerProps {
  timestamp: number;
}

export function DateDivider({ timestamp }: DateDividerProps) {
  return (
    <div className="my-4 flex items-center gap-3">
      <div className="h-px flex-1 bg-surface-200 dark:bg-surface-800" />
      <span className="text-xs text-surface-400">{dayLabel(timestamp)}</span>
      <div className="h-px flex-1 bg-surface-200 dark:bg-surface-800" />
    </div>
  );
}

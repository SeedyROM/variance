import { forwardRef } from "react";
import { cn } from "../../utils/cn";

interface IconButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  /** Highlight the button as active (e.g. a toggle that is on). */
  active?: boolean;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(
  ({ className, active, ...props }, ref) => (
    <button
      ref={ref}
      className={cn(
        "rounded-lg p-1.5 cursor-pointer transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary-500 disabled:pointer-events-none disabled:opacity-50",
        active
          ? "text-primary-500 bg-primary-500/10 hover:bg-primary-500/20"
          : "text-surface-500 hover:bg-surface-200 dark:hover:bg-surface-800",
        className
      )}
      {...props}
    />
  )
);

IconButton.displayName = "IconButton";

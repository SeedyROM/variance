import { forwardRef } from "react";
import { cn } from "../../utils/cn";

interface ScrollAreaProps {
  children: React.ReactNode;
  className?: string;
}

export const ScrollArea = forwardRef<HTMLDivElement, ScrollAreaProps>(
  ({ children, className }, ref) => (
    <div ref={ref} className={cn("overflow-y-auto overscroll-contain", className)}>
      {children}
    </div>
  )
);

ScrollArea.displayName = "ScrollArea";

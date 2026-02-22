import { ReactNode, useState, useRef, useEffect, useCallback } from "react";
import { createPortal } from "react-dom";
import { cn } from "../../utils/cn";

type Placement = "top" | "bottom" | "left" | "right";

interface TooltipProps {
  content: ReactNode;
  children: ReactNode;
  /** Milliseconds before the tooltip appears (default: 500) */
  delay?: number;
  /** Which side of the trigger to place the tooltip (default: "right") */
  placement?: Placement;
  /** Extra className on the outer wrapper */
  className?: string;
  /** Extra className on the tooltip popup itself */
  tooltipClassName?: string;
  /** Max width of the tooltip content (default: none) */
  maxWidth?: number;
}

const GAP = 8; // px between trigger and tooltip

function computePosition(
  rect: DOMRect,
  placement: Placement
): { top: number; left: number; transform: string } {
  switch (placement) {
    case "right":
      return {
        top: rect.top + rect.height / 2,
        left: rect.right + GAP,
        transform: "translateY(-50%)",
      };
    case "left":
      return {
        top: rect.top + rect.height / 2,
        left: rect.left - GAP,
        transform: "translate(-100%, -50%)",
      };
    case "top":
      return {
        top: rect.top - GAP,
        left: rect.left + rect.width / 2,
        transform: "translate(-50%, -100%)",
      };
    case "bottom":
    default:
      return {
        top: rect.bottom + GAP,
        left: rect.left + rect.width / 2,
        transform: "translateX(-50%)",
      };
  }
}

export function Tooltip({
  content,
  children,
  delay = 500,
  placement = "right",
  className,
  tooltipClassName,
  maxWidth,
}: TooltipProps) {
  const [visible, setVisible] = useState(false);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [pos, setPos] = useState({ top: 0, left: 0, transform: "" });
  const triggerRef = useRef<HTMLDivElement>(null);

  const show = useCallback(() => {
    timeoutRef.current = setTimeout(() => {
      if (triggerRef.current) {
        const rect = triggerRef.current.getBoundingClientRect();
        setPos(computePosition(rect, placement));
      }
      setVisible(true);
    }, delay);
  }, [delay, placement]);

  const hide = useCallback(() => {
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current);
      timeoutRef.current = null;
    }
    setVisible(false);
  }, []);

  useEffect(() => {
    return () => {
      if (timeoutRef.current) clearTimeout(timeoutRef.current);
    };
  }, []);

  return (
    <>
      <div
        ref={triggerRef}
        onMouseEnter={show}
        onMouseLeave={hide}
        className={cn("w-full", className)}
      >
        {children}
      </div>
      {visible &&
        createPortal(
          <div
            role="tooltip"
            className={cn(
              "fixed z-50 px-3 py-2.5 text-xs font-medium text-white rounded-lg pointer-events-none",
              "bg-surface-800 dark:bg-surface-700 border border-surface-600/50 shadow-xl shadow-black/30",
              tooltipClassName
            )}
            style={{
              top: `${pos.top}px`,
              left: `${pos.left}px`,
              transform: pos.transform,
              animation: "tooltip-in 0.15s ease-out",
              ...(maxWidth ? { maxWidth: `${maxWidth}px`, whiteSpace: "normal" as const } : {}),
            }}
          >
            {content}
          </div>,
          document.body
        )}
    </>
  );
}

import { ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { cn } from "../../utils/cn";

export interface ContextMenuItem {
  label: string;
  icon?: ReactNode;
  onClick: () => void;
  /** When true the item is rendered but greyed out and non-interactive. */
  disabled?: boolean;
  /** Visually separate this item from the ones above with a divider. */
  divider?: boolean;
}

interface ContextMenuProps {
  items: ContextMenuItem[];
  children: ReactNode;
  /** Extra className on the trigger wrapper. */
  className?: string;
}

/**
 * Right-click context menu rendered via portal at the cursor position.
 *
 * Usage:
 * ```tsx
 * <ContextMenu items={[{ label: "Copy", onClick: handleCopy }]}>
 *   <div>Right-click me</div>
 * </ContextMenu>
 * ```
 */
export function ContextMenu({ items, children, className }: ContextMenuProps) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const menuRef = useRef<HTMLDivElement>(null);

  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();

      // Position the menu at the cursor, but clamp to viewport edges.
      const x = Math.min(e.clientX, window.innerWidth - 200);
      const y = Math.min(e.clientY, window.innerHeight - items.length * 36 - 16);
      setPos({ x, y });
      setOpen(true);
    },
    [items.length]
  );

  // Close on any click outside or Escape
  useEffect(() => {
    if (!open) return;

    const onClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    // Use capture so we close before the click propagates
    document.addEventListener("mousedown", onClickOutside, true);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onClickOutside, true);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <>
      <div onContextMenu={handleContextMenu} className={className}>
        {children}
      </div>
      {open &&
        createPortal(
          <div
            ref={menuRef}
            className={cn(
              "fixed z-50 min-w-[160px] rounded-lg py-1",
              "bg-surface-50 dark:bg-surface-800",
              "border border-surface-200 dark:border-surface-700",
              "shadow-xl shadow-black/20"
            )}
            style={{
              top: `${pos.y}px`,
              left: `${pos.x}px`,
              animation: "tooltip-in 0.1s ease-out",
            }}
          >
            {items.map((item, i) => (
              <div key={i}>
                {item.divider && (
                  <div className="my-1 border-t border-surface-200 dark:border-surface-700" />
                )}
                <button
                  onClick={() => {
                    if (!item.disabled) {
                      item.onClick();
                      setOpen(false);
                    }
                  }}
                  disabled={item.disabled}
                  className={cn(
                    "flex w-full items-center gap-2.5 px-3 py-1.5 text-[13px] text-left transition-colors",
                    item.disabled
                      ? "text-surface-400 dark:text-surface-500 cursor-default"
                      : "text-surface-800 dark:text-surface-200 hover:bg-surface-200/60 dark:hover:bg-surface-700/60 cursor-pointer"
                  )}
                >
                  {item.icon && (
                    <span className="shrink-0 w-4 h-4 flex items-center justify-center">
                      {item.icon}
                    </span>
                  )}
                  {item.label}
                </button>
              </div>
            ))}
          </div>,
          document.body
        )}
    </>
  );
}

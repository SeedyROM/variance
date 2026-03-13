import { Dialog } from "./Dialog";
import { Button } from "./Button";

interface ConfirmDialogProps {
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  destructive?: boolean;
  loading?: boolean;
}

export function ConfirmDialog({
  open,
  onClose,
  onConfirm,
  title,
  message,
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  destructive = false,
  loading = false,
}: ConfirmDialogProps) {
  return (
    <Dialog open={open} onClose={onClose} title={title} className="max-w-sm">
      <p className="mb-6 text-sm text-surface-600 dark:text-surface-400">{message}</p>
      <div className="flex justify-end gap-2">
        <Button variant="secondary" size="sm" onClick={onClose} disabled={loading}>
          {cancelLabel}
        </Button>
        <Button
          variant={destructive ? "danger" : "primary"}
          size="sm"
          onClick={onConfirm}
          loading={loading}
        >
          {confirmLabel}
        </Button>
      </div>
    </Dialog>
  );
}

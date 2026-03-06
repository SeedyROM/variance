import { useState } from "react";
import QRCode from "react-qr-code";
import { Copy, Check } from "lucide-react";
import { Dialog } from "../ui/Dialog";

interface ShareContactModalProps {
  open: boolean;
  onClose: () => void;
  did: string;
  displayName: string | null;
}

function buildContactUri(did: string, displayName: string | null): string {
  const params = new URLSearchParams({ did });
  if (displayName) params.set("name", displayName);
  return `variance://add?${params.toString()}`;
}

export function ShareContactModal({ open, onClose, did, displayName }: ShareContactModalProps) {
  const [copied, setCopied] = useState(false);

  const uri = buildContactUri(did, displayName);

  function handleCopy() {
    void navigator.clipboard.writeText(uri).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  }

  return (
    <Dialog open={open} onClose={onClose} title="Share Contact">
      <div className="flex flex-col items-center gap-5">
        {/* QR code */}
        <div className="rounded-xl bg-white p-4 shadow-sm">
          <QRCode value={uri} size={200} />
        </div>

        {/* Identity info */}
        <div className="w-full space-y-1 text-center">
          {displayName && <p className="font-semibold text-primary-500 text-base">{displayName}</p>}
          <p className="break-all font-mono text-xs text-surface-500 dark:text-surface-400 leading-relaxed">
            {did}
          </p>
        </div>

        {/* Copy button */}
        <button
          onClick={handleCopy}
          className="flex w-full items-center justify-center gap-2 rounded-lg bg-primary-500 px-4 py-2 text-sm font-medium text-white hover:bg-primary-600 transition-colors"
        >
          {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
          {copied ? "Copied!" : "Copy contact link"}
        </button>

        <p className="text-xs text-surface-400 text-center">
          Share this QR code or link so others can add you on Variance.
        </p>
      </div>
    </Dialog>
  );
}

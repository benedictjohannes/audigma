import { AlertCircle, AlertTriangle, CheckCircle2, Info } from "lucide-react";
import { type ReactNode, useEffect, useRef } from "react";

export type Tone = "default" | "primary" | "success" | "warning" | "danger";

const TAG: Record<Tone, string> = {
  default: "border-line text-muted",
  primary: "border-primary text-primary bg-primary-bg",
  success: "border-success text-success bg-success-bg",
  warning: "border-warning text-warning bg-warning-bg",
  danger: "border-danger text-danger bg-danger-bg",
};

/** Ant-style Tag: a small bordered label. Colour is always paired with text. */
export function Tag({
  tone = "default",
  children,
  title,
}: {
  tone?: Tone;
  children: ReactNode;
  title?: string;
}) {
  return (
    <span
      title={title}
      className={`inline-flex items-center gap-1 rounded-sm border px-1.5 text-xs leading-5 whitespace-nowrap ${TAG[tone]}`}
    >
      {children}
    </span>
  );
}

const ALERT: Record<"error" | "warning" | "info" | "success", { box: string; icon: ReactNode }> = {
  error: {
    box: "border-danger bg-danger-bg",
    icon: <AlertCircle size={16} className="text-danger" aria-hidden />,
  },
  warning: {
    box: "border-warning bg-warning-bg",
    icon: <AlertTriangle size={16} className="text-warning" aria-hidden />,
  },
  info: {
    box: "border-primary bg-primary-bg",
    icon: <Info size={16} className="text-primary" aria-hidden />,
  },
  success: {
    box: "border-success bg-success-bg",
    icon: <CheckCircle2 size={16} className="text-success" aria-hidden />,
  },
};

/** Ant-style Alert: icon, optional bold lead, message. */
export function Alert({
  kind,
  children,
  label,
}: {
  kind: keyof typeof ALERT;
  children: ReactNode;
  label?: string;
}) {
  const a = ALERT[kind];
  return (
    <div
      role={kind === "error" ? "alert" : "status"}
      className={`flex gap-2 rounded-ant border px-3 py-2 ${a.box}`}
    >
      <span className="mt-0.5 shrink-0">{a.icon}</span>
      <div className="min-w-0">
        {label && <span className="mr-1 font-semibold">{label}</span>}
        {children}
      </div>
    </div>
  );
}

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <div className={`rounded-ant border border-line-soft bg-card ${className}`}>{children}</div>
  );
}

export function Switch({
  checked,
  onChange,
  label,
  id,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  id?: string;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-5.5 w-11 shrink-0 items-center rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${
        checked ? "bg-primary" : "bg-line"
      }`}
    >
      <span
        className={`inline-block h-4.5 w-4.5 rounded-full bg-white shadow transition-transform ${
          checked ? "translate-x-5.5" : "translate-x-0.5"
        }`}
      />
    </button>
  );
}

/** A centred dialog over a dimmed page. Escape and the backdrop close it. */
export function Modal({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    if (d && !d.open) d.showModal();
  }, []);
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click only; Escape is handled by onCancel
    <dialog
      ref={ref}
      aria-label={title}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      onClick={(e) => e.target === e.currentTarget && onClose()}
      className="m-auto max-h-[calc(100%-2rem)] w-full max-w-lg overflow-auto rounded-ant border border-line-soft bg-card p-4 text-inherit shadow-xl backdrop:bg-black/45"
    >
      <h2 className="mb-3 text-base font-semibold">{title}</h2>
      {children}
    </dialog>
  );
}

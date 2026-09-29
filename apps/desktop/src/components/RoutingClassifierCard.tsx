import { useState, type ReactNode } from "react";
import { Settings2 } from "lucide-react";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from "./ui/dialog";
import { Switch } from "./ui/switch";
import "./RoutingClassifierCard.css";

interface RoutingClassifierCardProps {
  id: string;
  title: string;
  summary: string;
  statusLabel: string;
  settingsLabel: string;
  enabled: boolean;
  disabled: boolean;
  busy: boolean;
  describedBy: string;
  onEnabledChange: (enabled: boolean) => void;
  children: ReactNode;
  feedback?: ReactNode;
}

export default function RoutingClassifierCard({
  id, title, summary, statusLabel, settingsLabel, enabled, disabled, busy,
  describedBy, onEnabledChange, children, feedback,
}: RoutingClassifierCardProps) {
  const [open, setOpen] = useState(false);
  const { copy } = useLocalizedCopy();

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <section className="routing-classifier-card" aria-label={title} data-enabled={enabled}>
        <div className="routing-classifier-heading">
          <Switch
            id={`${id}-enabled`} checked={enabled} disabled={disabled} aria-busy={busy}
            aria-labelledby={`${id}-title`} aria-describedby={describedBy}
            onCheckedChange={onEnabledChange}
          />
          <DialogTrigger asChild>
            <Button
              type="button" variant="ghost" className="routing-classifier-settings"
              aria-label={settingsLabel} aria-describedby={`${id}-summary-status`}
            >
              <span className="routing-classifier-copy">
                <span className="routing-classifier-title-row">
                  <span className="routing-classifier-title" id={`${id}-title`}>{title}</span>
                  <span className="routing-classifier-label" id={`${id}-summary-status`}>{statusLabel}</span>
                </span>
                <span className="routing-classifier-summary">{summary}</span>
              </span>
              <Settings2 className="routing-classifier-settings-icon" aria-hidden="true" />
            </Button>
          </DialogTrigger>
        </div>
        {!open && <div className="routing-classifier-details" hidden>{children}</div>}
        {!open && feedback}
      </section>
      {open && (
        <DialogContent className="routing-classifier-dialog" closeLabel={copy("Close", "关闭", "關閉", "閉じる")}>
          <DialogHeader className="routing-classifier-dialog-header">
            <DialogTitle>{settingsLabel}</DialogTitle>
            <DialogDescription>{summary}</DialogDescription>
          </DialogHeader>
          <div className="routing-classifier-dialog-toggle" data-enabled={enabled}>
            <div className="routing-classifier-copy">
              <span className="routing-classifier-title">{title}</span>
              <span className="routing-classifier-label">{statusLabel}</span>
            </div>
            <Switch
              id={`${id}-dialog-enabled`} checked={enabled} disabled={disabled} aria-busy={busy}
              aria-label={title} aria-describedby={describedBy} onCheckedChange={onEnabledChange}
            />
          </div>
          <div className="routing-classifier-details">{children}</div>
          {feedback}
        </DialogContent>
      )}
    </Dialog>
  );
}

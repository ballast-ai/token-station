import { useEffect, useMemo, useRef, useState } from "react";
import type { AgentInstallationView } from "../api";

interface InstallationPickerProps {
  agentName: string;
  installations: AgentInstallationView[];
  selectedPath: string;
  disabled?: boolean;
  onSelect: (path: string) => void;
}

function executableName(path: string) {
  const segments = path.split(/[\\/]/).filter(Boolean);
  return segments[segments.length - 1] ?? "安装";
}

function normalizedVersion(installation: AgentInstallationView) {
  const version = installation.discovery.version_normalized;
  if (!version) return null;
  return version.startsWith("v") ? version : `v${version}`;
}

const sourceLabels: Record<AgentInstallationView["discovery"]["binary_source"], string> = {
  homebrew: "Homebrew",
  npm_global: "npm 全局",
  path: "PATH",
  known_path: "已知目录",
  env_override: "环境变量",
};

function modifiedLabel(modifiedAtMs: number | null) {
  if (modifiedAtMs == null) return "修改时间未知";
  return `修改于 ${new Date(modifiedAtMs).toLocaleString()}`;
}

function hashLabel(hash: string | null) {
  return hash ? `SHA-256 ${hash.slice(0, 12)}` : "SHA-256 不可读";
}

export function installationLabels(installations: AgentInstallationView[]) {
  const baseLabels = installations.map((installation) => {
    const name = executableName(installation.discovery.canonical_path);
    const version = normalizedVersion(installation);
    return version ? `${name} · ${version}` : name;
  });
  const totals = new Map<string, number>();
  for (const label of baseLabels) totals.set(label, (totals.get(label) ?? 0) + 1);
  const positions = new Map<string, number>();

  return installations.map((installation, index) => {
    const baseLabel = baseLabels[index];
    const total = totals.get(baseLabel) ?? 1;
    const position = (positions.get(baseLabel) ?? 0) + 1;
    positions.set(baseLabel, position);
    return {
      path: installation.discovery.canonical_path,
      label: total > 1 ? `${baseLabel} · ${position}/${total}` : baseLabel,
    };
  });
}

export default function InstallationPicker({
  agentName,
  installations,
  selectedPath,
  disabled = false,
  onSelect,
}: InstallationPickerProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const options = useMemo(() => installationLabels(installations), [installations]);

  useEffect(() => setOpen(false), [agentName]);

  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  if (installations.length <= 1) return null;

  return (
    <div className="installation-picker" ref={rootRef}>
      <button
        className="btn installation-picker-trigger"
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
      >
        选择安装 <span aria-hidden="true">⌄</span>
      </button>
      {open && (
        <div className="installation-picker-menu" role="listbox" aria-label={`${agentName} 安装列表`}>
          {options.map((option, index) => {
            const selected = option.path === selectedPath;
            const discovery = installations[index].discovery;
            return (
              <button
                key={option.path}
                className={selected ? "selected" : ""}
                type="button"
                role="option"
                aria-label={option.label}
                aria-selected={selected}
                onClick={() => {
                  onSelect(option.path);
                  setOpen(false);
                }}
              >
                <span className="installation-option-title">{option.label}</span>
                <code className="installation-option-path">{discovery.canonical_path}</code>
                <span className="installation-option-facts">
                  {sourceLabels[discovery.binary_source]}
                  {discovery.is_path_default ? " · 当前生效" : ""}
                  {` · ${modifiedLabel(discovery.modified_at_ms)} · ${hashLabel(discovery.binary_sha256)}`}
                </span>
                {selected && <span className="installation-check" aria-hidden="true">✓</span>}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

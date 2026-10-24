import { useState } from "react";
import { setStartupScan } from "../../api";
import type { PendingScanInfo, StartupScanSetting } from "../../types";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./StartupScanModal.css";

type Choice = "classify_only" | "full" | "none";

type Props = {
  rootName: string;
  info: PendingScanInfo;
  /** There is an unfinished job for this root. */
  interrupted: boolean;
  onChoose: (choice: Choice) => void;
  onCancel: () => void;
};

function lastVerified(lastScanAt: number | null): string {
  if (!lastScanAt) return "nunca verificada";
  const date = new Date(lastScanAt * 1000);
  return `última verificação em ${date.toLocaleDateString()}`;
}

export default function StartupScanModal({
  rootName,
  info,
  interrupted,
  onChoose,
  onCancel,
}: Props) {
  const [choice, setChoice] = useState<Choice>(
    info.unclassified > 0 || interrupted ? "classify_only" : "full",
  );
  const [remember, setRemember] = useState(false);

  const options: { value: Choice; label: string; hint: string }[] = [
    ...(info.unclassified > 0 || interrupted
      ? [
          {
            value: "classify_only" as Choice,
            label: "Continuar classificação",
            hint: `${info.unclassified.toLocaleString()} arquivo(s) pendente(s) · usa o índice atual, sem reescanear`,
          },
        ]
      : []),
    {
      value: "full",
      label: "Verificar mudanças",
      hint: "descoberta completa da pasta (pode demorar em rede) + thumbnails + classificação",
    },
    {
      value: "none",
      label: "Só abrir",
      hint: "navega pelo índice atual; nada é processado",
    },
  ];

  async function confirm() {
    if (remember) {
      // Best effort: a settings.toml failure must not block opening the folder.
      try {
        await setStartupScan((choice === "none" ? "none" : choice) as StartupScanSetting);
      } catch {
        /* ignore */
      }
    }
    onChoose(choice);
  }

  return (
    <ModalOverlay onBackdropClick={onCancel}>
      <div
        className="modal-base startup-scan-modal"
        onClick={(e) => e.stopPropagation()}
        data-testid="startup-scan-modal"
      >
        <h3>"{rootName}" já está indexada</h3>
        <p className="startup-scan-subtitle">
          {info.unclassified.toLocaleString()} arquivo(s) sem classificação ·{" "}
          {lastVerified(info.lastScanAt)}
        </p>

        <div className="startup-scan-options">
          {options.map((option) => (
            <label className="startup-scan-option" key={option.value}>
              <input
                type="radio"
                name="startup-scan-choice"
                checked={choice === option.value}
                onChange={() => setChoice(option.value)}
              />
              <span className="startup-scan-option-body">
                <strong>{option.label}</strong>
                <span className="startup-scan-hint">{option.hint}</span>
              </span>
            </label>
          ))}
        </div>

        <label className="startup-scan-remember">
          <input
            type="checkbox"
            checked={remember}
            onChange={(e) => setRemember(e.target.checked)}
          />
          Não perguntar novamente (pode ser alterado em "Classification settings")
        </label>

        <div className="modal-actions">
          <button type="button" onClick={onCancel}>
            Cancelar
          </button>
          <button type="button" onClick={confirm}>
            Continuar
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}

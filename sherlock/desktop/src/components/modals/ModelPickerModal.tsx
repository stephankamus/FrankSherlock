import { useEffect, useRef, useState } from "react";
import {
  estimateVisionScan,
  getVisionSettings,
  listVisionModels,
  setVisionSettings,
  type VisionSettingsUpdate,
} from "../../api";
import type {
  VisionEstimate,
  VisionModelOption,
  VisionSettings,
} from "../../types";
import { errorMessage } from "../../utils";
import { formatBytes } from "../../utils/format";
import { formatEta } from "../../utils/scanEta";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./ModelPickerModal.css";

type Props = {
  /** When set, the projection is measured for this root. */
  rootPath?: string | null;
  onCancel: () => void;
  onConfirm: () => void;
  /** Lets the status bar refresh after a probe loaded/unloaded the model. */
  onRuntimeChanged?: () => void;
};

const RESOLUTIONS: { value: number; label: string; hint: string }[] = [
  {
    value: 0,
    label: "Original",
    hint: "sem redimensionar — muito mais lento; só compensa para texto muito fino",
  },
  {
    value: 1024,
    label: "1024 px",
    hint: "recomendado — mantém detalhe e sujeitos pequenos",
  },
  {
    value: 768,
    label: "768 px",
    hint: "25–40% mais rápido — pode perder sujeitos pequenos",
  },
];

const PROFILES: { value: "concise" | "detailed"; label: string; hint: string }[] = [
  {
    value: "concise",
    label: "Concisa",
    hint: "tipo + texto + descrição curta (~30 tokens)",
  },
  {
    value: "detailed",
    label: "Detalhada",
    hint: "inclui candidatos de série/personagem (~78 tokens, mais lento)",
  },
];

const OCR_ENGINES: { value: "off" | "llm" | "surya"; label: string; hint: string }[] = [
  { value: "off", label: "Desligado", hint: "não extrai texto" },
  { value: "llm", label: "Modelo de visão", hint: "rápido, recall menor" },
  { value: "surya", label: "Surya", hint: "melhor recall, muito mais lento" },
];

function sourceLabel(source: VisionEstimate["source"]): string {
  switch (source) {
    case "calibrated":
      return "✓ estimativa calibrada nesta máquina";
    case "cached":
      return "✓ medição em cache para estas opções";
    default:
      return "estimativa grosseira (±50%) — modelo indisponível";
  }
}

export default function ModelPickerModal({
  rootPath,
  onCancel,
  onConfirm,
  onRuntimeChanged,
}: Props) {
  const [settings, setSettings] = useState<VisionSettings | null>(null);
  const [models, setModels] = useState<VisionModelOption[]>([]);
  const [estimate, setEstimate] = useState<VisionEstimate | null>(null);
  const [loading, setLoading] = useState(true);
  const [estimating, setEstimating] = useState(false);
  const [elapsedS, setElapsedS] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const estimateSeq = useRef(0);
  const runtimeChanged = useRef(onRuntimeChanged);
  runtimeChanged.current = onRuntimeChanged;

  // Visible progress: the first calibration also loads the model (~1 min cold).
  useEffect(() => {
    if (!estimating) {
      setElapsedS(0);
      return;
    }
    const started = Date.now();
    const id = window.setInterval(
      () => setElapsedS(Math.round((Date.now() - started) / 1000)),
      1000,
    );
    return () => window.clearInterval(id);
  }, [estimating]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [current, installed] = await Promise.all([
          getVisionSettings(),
          listVisionModels(),
        ]);
        if (cancelled) return;
        setSettings(current);
        setModels(installed);
      } catch (err) {
        if (!cancelled) setError(errorMessage(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // The backend reads settings.toml, so re-estimate only after a successful save.
  useEffect(() => {
    if (!rootPath || !settings) return;
    const seq = ++estimateSeq.current;
    setEstimating(true);
    estimateVisionScan(rootPath)
      .then((result) => {
        if (seq !== estimateSeq.current) return;
        setEstimate(result);
        // The probes load the model, so VRAM and "current model" just changed.
        runtimeChanged.current?.();
      })
      .catch((err) => {
        if (seq === estimateSeq.current) setError(errorMessage(err));
      })
      .finally(() => {
        if (seq === estimateSeq.current) setEstimating(false);
      });
  }, [rootPath, settings]);

  async function apply(patch: Partial<VisionSettingsUpdate>) {
    if (!settings) return;
    const next: VisionSettingsUpdate = {
      model: settings.modelOverride,
      imageMaxSide: settings.imageMaxSide,
      promptProfile: settings.promptProfile,
      ocrEngine: settings.ocrEngine,
      ...patch,
    };
    try {
      const saved = await setVisionSettings(next);
      setSettings(saved);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  const modelValue = settings?.modelOverride ?? "";

  return (
    <ModalOverlay onBackdropClick={onCancel}>
      <div
        className="modal-base model-picker-modal"
        onClick={(e) => e.stopPropagation()}
        data-testid="model-picker-modal"
      >
        <h3>Configurar classificação</h3>
        <p className="model-picker-subtitle">
          Escolhas valem para o próximo scan. A projeção é medida com imagens da sua
          própria pasta.
        </p>

        {error && <p className="model-picker-error">{error}</p>}
        {loading && <p className="model-picker-loading">Carregando modelos…</p>}

        {!loading && settings && (
          <>
            <section className="model-picker-section">
              <h4>Modelo</h4>
              {models.length === 0 ? (
                <p className="model-picker-empty">
                  Nenhum modelo encontrado no Ollama. Instale um modelo para continuar.
                </p>
              ) : (
                <div className="model-picker-options">
                  <label className="model-picker-option">
                    <input
                      type="radio"
                      name="vision-model"
                      checked={modelValue === ""}
                      onChange={() => apply({ model: null })}
                    />
                    <span className="model-picker-option-body">
                      <strong>Automático</strong>
                      <span className="model-picker-hint">
                        escolhe o melhor modelo para o hardware detectado
                      </span>
                    </span>
                  </label>
                  {models.map((model) => (
                    <label className="model-picker-option" key={model.name}>
                      <input
                        type="radio"
                        name="vision-model"
                        checked={modelValue === model.name}
                        onChange={() => apply({ model: model.name })}
                      />
                      <span className="model-picker-option-body">
                        <strong>
                          {model.name} <span className="model-picker-size">{formatBytes(model.sizeBytes)}</span>
                          {model.recommended && <span className="model-picker-badge">recomendado</span>}
                          {model.active && <span className="model-picker-badge active">carregado</span>}
                        </strong>
                        <span
                          className={`model-picker-hint${model.fitsVram === false ? " warn" : ""}`}
                        >
                          {model.note}
                        </span>
                      </span>
                    </label>
                  ))}
                </div>
              )}
            </section>

            <section className="model-picker-section">
              <h4>Resolução enviada ao modelo</h4>
              <div className="model-picker-options">
                {RESOLUTIONS.map((option) => (
                  <label className="model-picker-option" key={option.value}>
                    <input
                      type="radio"
                      name="vision-resolution"
                      checked={settings.imageMaxSide === option.value}
                      onChange={() => apply({ imageMaxSide: option.value })}
                    />
                    <span className="model-picker-option-body">
                      <strong>{option.label}</strong>
                      <span className="model-picker-hint">{option.hint}</span>
                    </span>
                  </label>
                ))}
              </div>
            </section>

            <section className="model-picker-section">
              <h4>Descrição</h4>
              <div className="model-picker-options">
                {PROFILES.map((option) => (
                  <label className="model-picker-option" key={option.value}>
                    <input
                      type="radio"
                      name="vision-profile"
                      checked={settings.promptProfile === option.value}
                      onChange={() => apply({ promptProfile: option.value })}
                    />
                    <span className="model-picker-option-body">
                      <strong>{option.label}</strong>
                      <span className="model-picker-hint">{option.hint}</span>
                    </span>
                  </label>
                ))}
              </div>
            </section>

            <section className="model-picker-section">
              <h4>OCR (prints e documentos)</h4>
              <div className="model-picker-options">
                {OCR_ENGINES.map((option) => (
                  <label className="model-picker-option" key={option.value}>
                    <input
                      type="radio"
                      name="vision-ocr"
                      checked={settings.ocrEngine === option.value}
                      onChange={() => apply({ ocrEngine: option.value })}
                    />
                    <span className="model-picker-option-body">
                      <strong>{option.label}</strong>
                      <span className="model-picker-hint">{option.hint}</span>
                    </span>
                  </label>
                ))}
              </div>
            </section>

            <section className="model-picker-section model-picker-projection">
              <h4>Projeção da classificação</h4>
              {!rootPath && (
                <p className="model-picker-hint">
                  Abra pelo botão de scan de uma pasta para medir o tempo desta biblioteca.
                </p>
              )}
              {rootPath && estimating && !estimate && (
                <p className="model-picker-hint">
                  Medindo 3 imagens da pasta… ({elapsedS}s). Na primeira vez o Ollama
                  carrega o modelo, o que pode levar cerca de um minuto. Você pode
                  iniciar o scan sem esperar pela medição.
                </p>
              )}
              {rootPath && estimate && (
                <>
                  <p className="model-picker-total" data-testid="vision-estimate">
                    {estimate.counts.images.toLocaleString()} imagens ·{" "}
                    {estimate.perImageS.toFixed(2)} s/imagem ·{" "}
                    <strong>{formatEta(estimate.totalS)}</strong>
                    {estimating && (
                      <span className="model-picker-recalc">recalculando… ({elapsedS}s)</span>
                    )}
                  </p>
                  <p className="model-picker-hint">
                    {sourceLabel(estimate.source)} · {estimate.samples} amostra(s) ·
                    preparo {estimate.breakdown.gapS.toFixed(2)}s + prefill{" "}
                    {estimate.breakdown.prefillS.toFixed(2)}s + decode{" "}
                    {estimate.breakdown.decodeS.toFixed(2)}s
                  </p>
                  {estimate.counts.videos > 0 && (
                    <p className="model-picker-hint">
                      {estimate.counts.videos.toLocaleString()} vídeo(s) e{" "}
                      {estimate.counts.pdfs.toLocaleString()} PDF(s) também entram na
                      projeção.
                    </p>
                  )}
                  {estimate.spillBytes > 0 && (
                    <p className="model-picker-warn">
                      {formatBytes(estimate.spillBytes)} do modelo ficaram na RAM — o
                      decode fica ~2× mais lento. Um modelo menor ou uma resolução menor
                      resolve.
                    </p>
                  )}
                </>
              )}
            </section>
          </>
        )}

        <div className="modal-actions">
          <button type="button" onClick={onCancel}>
            Cancelar
          </button>
          <button type="button" onClick={onConfirm} disabled={loading}>
            {rootPath ? "Iniciar classificação" : "Fechar"}
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}

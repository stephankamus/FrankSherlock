import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./MediaOfflineModal.css";

type Props = {
  /** Name of the folder whose media server stopped answering. */
  rootName: string;
  /** Stops the pass: a held pass is still a running scan job. */
  onPause: () => void;
  /** Hides the dialog without touching the wait. */
  onHide: () => void;
};

/**
 * Shown while a pass holds itself for a media server that went away.
 *
 * Deliberately **not** a blocking alert: the wait has no timeout, so a dialog that could
 * only be dismissed by pausing would freeze the app for as long as the share is down —
 * including the settings screen. Hiding it keeps the wait running, and the root card
 * stays marked offline either way.
 */
export default function MediaOfflineModal({ rootName, onPause, onHide }: Props) {
  return (
    <ModalOverlay onBackdropClick={onHide}>
      <div
        className="modal-base media-offline-modal"
        onClick={(e) => e.stopPropagation()}
        data-testid="media-offline-modal"
      >
        <h3>Servidor de mídia desconectado</h3>
        <p>
          "{rootName}" parou de responder. O passe está aguardando a reconexão e nada foi
          perdido: os arquivos que falharam no momento da queda continuam pendentes para a
          próxima rodada.
        </p>
        <p className="media-offline-hint">
          A pasta é verificada a cada poucos segundos, sem prazo para desistir. O
          processamento retoma sozinho quando ela voltar.
        </p>
        <div className="modal-actions">
          <button type="button" onClick={onHide}>Ocultar (continua tentando)</button>
          <button
            type="button"
            className="media-offline-pause"
            onClick={onPause}
          >
            Pausar agora
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}

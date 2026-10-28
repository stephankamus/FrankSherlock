/**
 * The overlay moved to `@frank-sherlock/shared` so shared modals can use it.
 *
 * This re-export keeps the desktop's twenty-odd modals importing the same path;
 * the implementation (and the Escape stack) lives in the shared package.
 */
export { ModalOverlay as default } from "@frank-sherlock/shared";

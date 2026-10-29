import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  LibraryHttpError,
  LibraryProvider,
  errorMessage,
} from "@frank-sherlock/shared";
import App from "./App";
import {
  clearToken,
  createWebClient,
  resolveToken,
  saveToken,
  stripTokenFromUrl,
} from "./config";

type GateState = "checking" | "ready" | "login";

/**
 * Sign-in gate for the read-only web client.
 *
 * The token still works through `?token=`, but pasting it into the address bar was the
 * only way in. This component probes the API with whatever token it has: a 200 (or any
 * non-auth failure, which the app shell already explains) opens the library, a 401 shows
 * the form. The token is only stored after the server accepts it, and a later 401 drops
 * the stored value and brings the form back.
 */
export default function TokenGate() {
  const [token, setToken] = useState(resolveToken);
  const [state, setState] = useState<GateState>("checking");
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const requestId = useRef(0);

  const onUnauthorized = useCallback(() => {
    clearToken();
    setToken("");
    setState("login");
    setError("The token was rejected. Enter it again.");
  }, []);

  const client = useMemo(
    () => createWebClient(token, onUnauthorized),
    [token, onUnauthorized],
  );

  // The URL form was already copied into storage by `resolveToken`.
  useEffect(() => {
    stripTokenFromUrl();
  }, []);

  useEffect(() => {
    const id = ++requestId.current;
    setState("checking");
    client
      .health?.()
      .then(() => {
        if (id === requestId.current) setState("ready");
      })
      .catch((err) => {
        if (id !== requestId.current) return;
        if (err instanceof LibraryHttpError && err.status === 401) {
          setState("login");
          setError(token ? "Invalid token. Enter it again." : null);
          return;
        }
        // A missing library, an unmapped root or a down server is not an auth problem:
        // let the shell render and show the error where it belongs.
        setState("ready");
      });
  }, [client, token]);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    const candidate = input.trim();
    if (!candidate) {
      setError("Enter the access token.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      // Validate before storing: a rejected token must not replace a working one.
      const probe = createWebClient(candidate);
      await probe.health?.();
      saveToken(candidate);
      setInput("");
      setToken(candidate);
    } catch (err) {
      setError(
        err instanceof LibraryHttpError && err.status === 401
          ? "Invalid token."
          : errorMessage(err),
      );
    } finally {
      setSubmitting(false);
    }
  }

  if (state === "checking") {
    return (
      <div className="auth-screen">
        <div className="auth-card" role="status">
          Connecting…
        </div>
      </div>
    );
  }

  if (state === "login") {
    return (
      <div className="auth-screen">
        <form className="auth-card" onSubmit={handleSubmit}>
          <h1>Frank Sherlock</h1>
          <p>Enter the access token configured for this library.</p>
          <label htmlFor="access-token">Access token</label>
          <input
            id="access-token"
            type="password"
            value={input}
            onChange={(event) => setInput(event.target.value)}
            autoComplete="current-password"
            autoFocus
            spellCheck={false}
          />
          {error && (
            <p className="auth-error" role="alert">
              {error}
            </p>
          )}
          <button type="submit" disabled={submitting}>
            {submitting ? "Checking…" : "Sign in"}
          </button>
          <p className="auth-hint">
            A link with <code>?token=…</code> still signs you in once and stores the token
            on this device.
          </p>
        </form>
      </div>
    );
  }

  return (
    <LibraryProvider client={client}>
      <App />
    </LibraryProvider>
  );
}

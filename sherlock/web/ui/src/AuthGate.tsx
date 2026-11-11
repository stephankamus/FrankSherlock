import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { LibraryProvider } from "@frank-sherlock/shared";

import App from "./App";
import { fetchMe, login as loginUser, logout, type WebSession } from "./auth";
import {
  clearToken,
  createWebClient,
  resolveToken,
  saveToken,
  stripTokenFromUrl,
} from "./config";

type GateState = "checking" | "ready" | "login";
type LoginMode = "user" | "token";

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function friendlyError(error: unknown): string {
  const message = messageOf(error);
  if (message === "invalid credentials") return "Invalid login or password.";
  if (message === "unauthorized") return "Invalid token.";
  return message;
}

/**
 * Sign-in gate for the read-only web client.
 *
 * Two ways in, one screen:
 *
 * 1. a user account created in the desktop app, carried by an HttpOnly cookie;
 * 2. the optional `AUTH_TOKEN`, stored in localStorage and appended to media URLs.
 *
 * On load the component probes the cookie session first, then a `?token=` or the
 * previously stored token. A `401` from any library call brings the form back.
 */
export default function AuthGate() {
  const [session, setSession] = useState<WebSession | null>(null);
  /** Only meaningful in token mode; the user session lives in the cookie. */
  const [token, setToken] = useState("");
  const [state, setState] = useState<GateState>("checking");
  const [mode, setMode] = useState<LoginMode>("user");
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [tokenInput, setTokenInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const requestId = useRef(0);

  const onUnauthorized = useCallback(() => {
    clearToken();
    setToken("");
    setSession(null);
    setState("login");
    setMode("user");
    setError("Your session expired. Sign in again.");
  }, []);

  const client = useMemo(
    () => createWebClient(session?.kind === "token" ? token : "", onUnauthorized),
    [session, token, onUnauthorized],
  );

  useEffect(() => {
    let cancelled = false;
    const id = ++requestId.current;
    // `resolveToken` stores the `?token=` value before `stripTokenFromUrl` removes it.
    const candidate = resolveToken();
    stripTokenFromUrl();

    async function probe() {
      try {
        const me = await fetchMe();
        if (cancelled || id !== requestId.current) return;
        if (me) {
          setSession(me);
          setToken("");
          setState("ready");
          return;
        }
      } catch (err: unknown) {
        if (cancelled || id !== requestId.current) return;
        setError(messageOf(err));
      }

      if (candidate) {
        try {
          const me = await fetchMe(candidate);
          if (cancelled || id !== requestId.current) return;
          if (me) {
            saveToken(candidate);
            setToken(candidate);
            setSession(me);
            setState("ready");
            return;
          }
          clearToken();
        } catch (err: unknown) {
          if (cancelled || id !== requestId.current) return;
          setError(messageOf(err));
        }
      }

      if (!cancelled && id === requestId.current) setState("login");
    }

    void probe();
    return () => {
      cancelled = true;
    };
  }, []);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    if (submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      if (mode === "user") {
        const me = await loginUser(login.trim(), password);
        setSession(me);
        setPassword("");
        setState("ready");
        return;
      }

      const candidate = tokenInput.trim();
      if (!candidate) {
        setError("Enter the access token.");
        return;
      }
      const me = await fetchMe(candidate);
      if (!me) {
        setError("Invalid token.");
        return;
      }
      saveToken(candidate);
      setToken(candidate);
      setSession(me);
      setTokenInput("");
      setState("ready");
    } catch (err: unknown) {
      setError(friendlyError(err));
    } finally {
      setSubmitting(false);
    }
  }

  function switchMode() {
    setMode((current) => (current === "user" ? "token" : "user"));
    setError(null);
    setPassword("");
    setTokenInput("");
  }

  function handleSignOut() {
    const wasUser = session?.kind === "user";
    clearToken();
    setToken("");
    setSession(null);
    setState("login");
    setMode("user");
    setError(null);
    if (wasUser) void logout();
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
          <p>
            {mode === "user"
              ? "Sign in with the account created in the desktop app."
              : "Enter the access token configured for this deployment."}
          </p>

          {mode === "user" ? (
            <>
              <label htmlFor="web-login">Login</label>
              <input
                id="web-login"
                type="text"
                value={login}
                onChange={(event) => setLogin(event.target.value)}
                autoComplete="username"
                spellCheck={false}
                autoFocus
                disabled={submitting}
              />
              <label htmlFor="web-password">Password</label>
              <input
                id="web-password"
                type="password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                autoComplete="current-password"
                disabled={submitting}
              />
            </>
          ) : (
            <>
              <label htmlFor="access-token">Access token</label>
              <input
                id="access-token"
                type="password"
                value={tokenInput}
                onChange={(event) => setTokenInput(event.target.value)}
                autoComplete="off"
                spellCheck={false}
                autoFocus
                disabled={submitting}
              />
            </>
          )}

          {error && (
            <p className="auth-error" role="alert">
              {error}
            </p>
          )}
          <button type="submit" disabled={submitting}>
            {submitting ? "Checking…" : "Sign in"}
          </button>
          <button type="button" className="auth-switch" onClick={switchMode} disabled={submitting}>
            {mode === "user" ? "Use an access token instead" : "Use a user account instead"}
          </button>
          {mode === "token" && (
            <p className="auth-hint">
              A link with <code>?token=…</code> still signs you in once and stores the
              token on this device.
            </p>
          )}
        </form>
      </div>
    );
  }

  return (
    <LibraryProvider client={client}>
      <App session={session} onSignOut={handleSignOut} />
    </LibraryProvider>
  );
}

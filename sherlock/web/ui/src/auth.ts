/**
 * The small JSON surface of `/api/auth`.
 *
 * User sessions live in an HttpOnly cookie, so this module never stores a user token
 * in JavaScript. Token mode is the optional `AUTH_TOKEN` path and is passed as a
 * bearer header only for the probe; the library client appends it to media URLs.
 */

export type WebSession =
  | { kind: "user"; id: number; name: string; login: string }
  | { kind: "token"; name: string };

async function request(path: string, init: RequestInit = {}): Promise<Response> {
  return fetch(`/api/auth${path}`, {
    ...init,
    credentials: "same-origin",
    headers: {
      Accept: "application/json",
      ...(init.headers as Record<string, string> | undefined),
    },
  });
}

async function readError(response: Response): Promise<string> {
  try {
    const body: unknown = await response.json();
    if (body && typeof body === "object" && "error" in body) {
      return String((body as { error: unknown }).error);
    }
  } catch {
    /* a non-JSON error page is still an error */
  }
  return `request failed with status ${response.status}`;
}

/**
 * Current principal, or `null` when the request is unauthorized.
 *
 * Without `token` this probes the session cookie. With `token` it probes the optional
 * API token, which is how the login screen validates before storing it.
 */
export async function fetchMe(token?: string): Promise<WebSession | null> {
  const headers: Record<string, string> = {};
  if (token) headers.Authorization = `Bearer ${token}`;
  const response = await request("/me", { headers });
  if (response.status === 401) return null;
  if (!response.ok) throw new Error(await readError(response));
  return (await response.json()) as WebSession;
}

/** Sign in with a user account. The server sets the HttpOnly cookie. */
export async function login(login: string, password: string): Promise<WebSession> {
  const response = await request("/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ login, password }),
  });
  if (!response.ok) throw new Error(await readError(response));
  return (await response.json()) as WebSession;
}

/** Clear the session cookie on the server. Best-effort: the UI returns to login. */
export async function logout(): Promise<void> {
  try {
    await request("/logout", { method: "POST" });
  } catch {
    /* the local session state is already gone */
  }
}

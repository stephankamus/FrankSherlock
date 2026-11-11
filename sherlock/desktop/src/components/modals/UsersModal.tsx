import { useCallback, useEffect, useState, type FormEvent } from "react";

import {
  createUser,
  deleteUser,
  listUsers,
  setUserPassword,
  updateUser,
} from "../../api";
import type { User } from "../../types";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./SettingsModal.css";
import "./UsersModal.css";

type Props = {
  onClose: () => void;
  /** The database itself is read-only; the screen still lists what is stored. */
  readOnly?: boolean;
};

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function formatDate(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toISOString().slice(0, 10);
}

/**
 * Accounts that can sign in to the web app.
 *
 * The web server only reads the published snapshot, so every change made here reaches
 * the browser on the next publish. The password field is write-only: the backend never
 * returns a hash, and this screen never displays one.
 */
export default function UsersModal({ onClose, readOnly = false }: Props) {
  const [users, setUsers] = useState<User[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [editing, setEditing] = useState<User | null>(null);
  const [name, setName] = useState("");
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");

  const [passwordUser, setPasswordUser] = useState<User | null>(null);
  const [newPassword, setNewPassword] = useState("");
  const [deleting, setDeleting] = useState<User | null>(null);

  const reload = useCallback(async () => {
    try {
      setUsers(await listUsers());
      setError(null);
    } catch (err: unknown) {
      setError(messageOf(err));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  function resetForm() {
    setEditing(null);
    setName("");
    setLogin("");
    setPassword("");
  }

  function startEdit(user: User) {
    setEditing(user);
    setName(user.name);
    setLogin(user.login);
    setPassword("");
    setPasswordUser(null);
    setDeleting(null);
    setError(null);
  }

  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      await reload();
    } catch (err: unknown) {
      setError(messageOf(err));
    } finally {
      setBusy(false);
    }
  }

  async function submitForm(event: FormEvent) {
    event.preventDefault();
    if (readOnly || busy) return;
    if (editing) {
      await run(async () => {
        await updateUser(editing.id, name, login);
        resetForm();
      });
      return;
    }
    await run(async () => {
      await createUser(name, login, password);
      resetForm();
    });
  }

  async function submitPassword(event: FormEvent) {
    event.preventDefault();
    if (!passwordUser || readOnly || busy) return;
    await run(async () => {
      await setUserPassword(passwordUser.id, newPassword);
      setPasswordUser(null);
      setNewPassword("");
    });
  }

  async function confirmDelete() {
    if (!deleting || readOnly || busy) return;
    const target = deleting;
    await run(async () => {
      await deleteUser(target.id);
      setDeleting(null);
      if (editing?.id === target.id) resetForm();
    });
  }

  return (
    <ModalOverlay onBackdropClick={onClose}>
      <div className="modal-base settings-modal users-modal" onClick={(e) => e.stopPropagation()}>
        <h3>Web users</h3>
        <p className="settings-hint">
          Accounts that can sign in to the web app. Changes reach the web with the next
          library publish.
        </p>

        {readOnly && (
          <p className="users-warning" role="status">
            The database is read-only, so accounts cannot be changed here.
          </p>
        )}
        {error && (
          <p className="settings-error" role="alert">
            {error}
          </p>
        )}

        <form className="users-form" onSubmit={submitForm}>
          <h4>{editing ? `Edit ${editing.name}` : "Create user"}</h4>
          <div className="settings-field">
            <label htmlFor="users-name">Name</label>
            <input
              id="users-name"
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoComplete="off"
              disabled={readOnly || busy}
            />
          </div>
          <div className="settings-field">
            <label htmlFor="users-login">Login</label>
            <input
              id="users-login"
              type="text"
              value={login}
              onChange={(e) => setLogin(e.target.value)}
              autoComplete="username"
              spellCheck={false}
              disabled={readOnly || busy}
            />
          </div>
          {!editing && (
            <div className="settings-field">
              <label htmlFor="users-password">Password</label>
              <input
                id="users-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete="new-password"
                disabled={readOnly || busy}
              />
              <span className="settings-hint">At least 8 characters.</span>
            </div>
          )}
          <div className="modal-actions">
            <button type="submit" disabled={readOnly || busy}>
              {busy ? "Saving…" : editing ? "Save changes" : "Create user"}
            </button>
            {editing && (
              <button type="button" onClick={resetForm} disabled={busy}>
                Cancel
              </button>
            )}
          </div>
        </form>

        <div className="users-list-wrap">
          <h4>Existing users</h4>
          {users === null && <p className="settings-loading">Loading…</p>}
          {users && users.length === 0 && (
            <p className="users-empty">
              No users yet. Create the first account; the web can only sign in after a
              publish.
            </p>
          )}
          {users && users.length > 0 && (
            <ul className="users-list">
              {users.map((user) => (
                <li
                  key={user.id}
                  className={`users-row${editing?.id === user.id ? " users-row-editing" : ""}`}
                >
                  <div className="users-identity">
                    <strong>{user.name}</strong>
                    <span>{user.login}</span>
                  </div>
                  <span className="users-created" title="Created">
                    {formatDate(user.createdAt)}
                  </span>
                  <div className="users-actions">
                    <button
                      type="button"
                      onClick={() => startEdit(user)}
                      disabled={readOnly || busy}
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setPasswordUser(user);
                        setNewPassword("");
                        setDeleting(null);
                        setError(null);
                      }}
                      disabled={readOnly || busy}
                    >
                      Change password
                    </button>
                    <button
                      type="button"
                      className="users-danger"
                      onClick={() => {
                        setDeleting(user);
                        setPasswordUser(null);
                        setError(null);
                      }}
                      disabled={readOnly || busy}
                    >
                      Delete
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>

        {passwordUser && (
          <form className="users-panel" onSubmit={submitPassword}>
            <h4>Change password for {passwordUser.name}</h4>
            <div className="settings-field">
              <label htmlFor="users-new-password">New password</label>
              <input
                id="users-new-password"
                type="password"
                value={newPassword}
                onChange={(e) => setNewPassword(e.target.value)}
                autoComplete="new-password"
                disabled={readOnly || busy}
                autoFocus
              />
            </div>
            <div className="modal-actions">
              <button type="submit" disabled={readOnly || busy}>
                {busy ? "Saving…" : "Set password"}
              </button>
              <button
                type="button"
                onClick={() => {
                  setPasswordUser(null);
                  setNewPassword("");
                }}
                disabled={busy}
              >
                Cancel
              </button>
            </div>
          </form>
        )}

        {deleting && (
          <div className="users-panel users-danger-panel" role="alertdialog" aria-label="Delete user">
            <h4>Delete {deleting.name}?</h4>
            <p>
              The account stops working after the next publish. If this is the last
              account and the deployment has no AUTH_TOKEN, the web app cannot be entered
              until another user is created and published.
            </p>
            <div className="modal-actions">
              <button type="button" className="users-danger" onClick={() => void confirmDelete()} disabled={busy}>
                {busy ? "Deleting…" : "Delete user"}
              </button>
              <button type="button" onClick={() => setDeleting(null)} disabled={busy}>
                Cancel
              </button>
            </div>
          </div>
        )}

        {users && users.length <= 1 && (
          <p className="users-warning">
            If this is the last account and the deployment has no AUTH_TOKEN, the web app
            cannot be entered until another user is published.
          </p>
        )}

        <div className="modal-actions users-footer">
          <button type="button" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}

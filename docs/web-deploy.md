# Deploying the Frank Sherlock web client

The web app is a read-only view of a library published by the desktop. The desktop
still owns the pipeline; the container only reads a snapshot.

## 1. Publish from the desktop

Settings → **Publicação**:

- pick a folder on the network share (`publish_dir`) — it must not be inside a photo
  root;
- keep "Publish after every scan" on, and pick how many revisions to keep (default 2).

The folder ends up like this:

```
current.json
index.20261027T190200Z.sqlite
thumbs/<root_key>/<rel_path>.jpg
face_crops/<file>.jpg
```

`current.json` is the only mutable file: it is written last, atomically.

## 2. Media map

The snapshot keeps the absolute paths of the machine that scanned the library. The
container resolves them through `config/media.map.json`:

```bash
cp config/media.map.example.json config/media.map.json
```

```json
[
  { "storedPath": "/home/stephan/Fotos", "mountedPath": "/mnt/fotos" },
  { "storedPath": "/mnt/nas/Anime",      "mountedPath": "/mnt/anime" }
]
```

`config/media.map.json` is git-ignored because it carries the absolute paths of the
machine that ran the scan. A second root only needs a new pair here plus the matching
bind mount in docker-compose.yml.

Every `mountedPath` must be a directory **inside the container**. A root with no entry
is not fatal: it shows up in `/api/health` as `unmappedRoots` and as a banner in the UI,
and its photos answer `409` when requested.

### SMB share: cifs mount or GVFS

`storedPath` is what the desktop recorded **at scan time**. If you scanned through
Files (GVFS) it is a `/run/user/1000/gvfs/smb-share:server=…,share=…` path; if you
scanned through a kernel mount it is the `/mnt/…` path. Keep that in mind when filling
the media map after switching.

**Recommended — one kernel mount, independent of your login session:**

```bash
sudo apt install cifs-utils
sudo install -m 0600 -o root -g root /dev/null /etc/samba/openmediavault.cred
sudo tee /etc/samba/openmediavault.cred >/dev/null <<'EOF'
username=stephan
password=…
EOF
sudo mkdir -p /mnt/omv-desenvolvimento
sudo mount -t cifs //openmediavault.local/desenvolvimento /mnt/omv-desenvolvimento \
  -o credentials=/etc/samba/openmediavault.cred,uid=1000,gid=1000,iocharset=utf8,vers=3.1.1,ro
```

Persist it in `/etc/fstab` so the container can start before you log in:

```
//openmediavault.local/desenvolvimento /mnt/omv-desenvolvimento cifs credentials=/etc/samba/openmediavault.cred,uid=1000,gid=1000,iocharset=utf8,vers=3.1.1,ro,nofail,x-systemd.automount 0 0
```

Then `.env` points at the mounted tree:

```bash
PUBLISH_DIR=/mnt/omv-desenvolvimento/FrankSherlock
PHOTOS_DIR=/mnt/omv-fotos/Fotos
```

**Quick path — the mount Files already made (no sudo):** a GVFS path contains colons,
which the short `src:dst:ro` volume syntax parses as "too many colons", so
docker-compose.yml uses the long form:

```yaml
      - type: bind
        source: ${PUBLISH_DIR}
        target: /library
        read_only: true
```

Two caveats remain: `/run/user/1000` is mode `0700`, so the container — which runs as
uid 10001 — cannot even traverse it. Run it as your own uid instead:

```bash
SHERLOCK_UID=1000
SHERLOCK_GID=1000
```

(`docker-compose.yml` feeds those into `user:`; the default stays `10001:10001`, the
image's own user, once the share is a cifs mount.) And the GVFS mount disappears when
you log out, so `restart: unless-stopped` cannot bring the container back after a
reboot — which is why the cifs mount is the recommendation.

## 3. Run it

```bash
cd sherlock/web
cp .env.example .env       # set the host paths; AUTH_TOKEN is optional
docker compose up -d --build
```

`.env` holds the optional API token and the `COOKIE_SECURE` switch, so it is git-ignored;
`.env.example` documents every variable compose consumes. Inline overrides still work
when you do not want a file:

```bash
# Only needed for script/API access, or before the first user is published.
AUTH_TOKEN="$(openssl rand -hex 24)" \
PUBLISH_DIR=/mnt/publish \
PHOTOS_DIR=/mnt/fotos \
docker compose up -d --build
```

Or from a prebuilt image:

```bash
docker run -d --name sherlock-web \
  --read-only --tmpfs /tmp \
  -p 8080:8080 \
  -e AUTH_TOKEN=... \
  -e COOKIE_SECURE=1 \
  -v /mnt/publish:/library:ro \
  -v /mnt/fotos:/mnt/fotos:ro \
  -v "$PWD/config:/config:ro" \
  ghcr.io/OWNER/frank-sherlock-web:latest
```

| Variable | Meaning |
|---|---|
| `LIBRARY_DIR` | mount of the publish folder (`/library`) |
| `MEDIA_MAP` | JSON mapping stored root paths to container mounts |
| `AUTH_TOKEN` | optional read-only API token; empty means only published user accounts can enter |
| `COOKIE_SECURE` | mark the session cookie `Secure`; also inferred from `X-Forwarded-Proto: https` |
| `CACHE_DIR` | writable dir for `?w=` derivatives (tmpfs, disposable) |
| `PORT` | listener port (default 8080) |
| `LIBRARY_COPY_TO_LOCAL` | reserved for slow shares (not implemented yet) |

The container runs as a non-root user, with a read-only root filesystem; only `/tmp`
is writable.

## 4. Accounts, tokens and TLS

Create the web accounts in the desktop app: open **Web users** in the sidebar, create at
least one account and then publish (**Settings → Publicação**). The users table travels
inside the same snapshot as the index, so a user change reaches the browser only after
the next publish. The web server never writes users and never receives a plaintext
password.

The login screen accepts a **login and password**. Passwords are stored as Argon2id
hashes. A successful login creates an in-memory session and sets an `HttpOnly`,
`SameSite=Strict` cookie; the browser sends it automatically for JSON and media requests,
so user sessions do not put a secret in a URL. Sessions are lost when the container
restarts. A publish that deletes a user or changes a password invalidates that user's
session on the next request.

`AUTH_TOKEN` remains optional and **read-only**. It is accepted as
`Authorization: Bearer …` or `?token=…` and is useful for scripts and for entering the
site before the first user is published. It does not grant any account-management or
write capability. The `?token=` link still signs in once, is stored in `localStorage` and
is removed from the address bar; token mode appends the token to media URLs, so those
URLs end up in proxy logs.

The user session cookie is an additional barrier, not the primary one: put a reverse
proxy in front (Caddy, Traefik, nginx) with Basic Auth or Authentik and TLS. Set
`COOKIE_SECURE=1` when the proxy does not send `X-Forwarded-Proto: https`. Without TLS,
traffic on the LAN can be read by anyone on the path.

## 5. Install on the phone

Open the site in Chrome/Safari and "Add to Home Screen". The manifest and the app-shell
service worker make it start standalone; `/api` is always network-only, there is no
offline library.

## 6. Troubleshooting

| Symptom | Check |
|---|---|
| 503 on every request | `current.json` missing: has the desktop published at least once? |
| 503 with `No such file or directory`, or `/library` empty inside the container, right after a successful publish | The container was created before the host share was mounted: bind sources are resolved at create time. Run `docker compose up -d --force-recreate` after the `mount -t cifs` (Docker Desktop keeps an empty placeholder until then) |
| Banner lists a root | Add it to `media.map.json` and mount it in compose |
| 401 | Sign in again. If the account was just created, publish the snapshot; if using `AUTH_TOKEN`, it differs from the stored value or a stale `?token=` is still in the browser |
| Videos do not seek | The share must allow byte ranges end to end (SMB/NFS both do) |
| Stale thumbnails | Publish again; the derivative cache is namespaced by revision |
| Login says no users are published | Create an account in **Web users** and publish the library |
| Login says the revision has no users table | The desktop build that published it predates accounts; update the desktop and publish again |
| A newly created user cannot log in | Publish the snapshot after creating or changing the account |
| `--health` fails | The container cannot read `LIBRARY_DIR` or its pointer |

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

```json
[
  { "storedPath": "/home/stephan/Fotos", "mountedPath": "/mnt/fotos" },
  { "storedPath": "/mnt/nas/Anime",      "mountedPath": "/mnt/anime" }
]
```

Every `mountedPath` must be a directory **inside the container**. A root with no entry
is not fatal: it shows up in `/api/health` as `unmappedRoots` and as a banner in the UI,
and its photos answer `409` when requested.

## 3. Run it

```bash
cd sherlock/web
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
  -v /mnt/publish:/library:ro \
  -v /mnt/fotos:/mnt/fotos:ro \
  -v "$PWD/config:/config:ro" \
  ghcr.io/OWNER/frank-sherlock-web:latest
```

| Variable | Meaning |
|---|---|
| `LIBRARY_DIR` | mount of the publish folder (`/library`) |
| `MEDIA_MAP` | JSON mapping stored root paths to container mounts |
| `AUTH_TOKEN` | bearer token for `/api`; empty disables auth and logs a warning |
| `CACHE_DIR` | writable dir for `?w=` derivatives (tmpfs, disposable) |
| `PORT` | listener port (default 8080) |
| `LIBRARY_COPY_TO_LOCAL` | reserved for slow shares (not implemented yet) |

The container runs as a non-root user, with a read-only root filesystem; only `/tmp`
is writable.

## 4. Auth and TLS

The app token is a second barrier, not the primary one: put a reverse proxy in front
(Caddy, Traefik, nginx) with Basic Auth or Authentik and TLS. The token can be passed as
`Authorization: Bearer …` or `?token=…` (the query form is what makes `<img>` and
`<video>` work without custom headers). Open the app once as
`https://photos.example.com/?token=…`; the client stores it in `localStorage` and removes
it from the address bar.

## 5. Install on the phone

Open the site in Chrome/Safari and "Add to Home Screen". The manifest and the app-shell
service worker make it start standalone; `/api` is always network-only, there is no
offline library.

## 6. Troubleshooting

| Symptom | Check |
|---|---|
| 503 on every request | `current.json` missing: has the desktop published at least once? |
| Banner lists a root | Add it to `media.map.json` and mount it in compose |
| 401 | `AUTH_TOKEN` differs from the one the client stored; reopen with `?token=` |
| Videos do not seek | The share must allow byte ranges end to end (SMB/NFS both do) |
| Stale thumbnails | Publish again; the derivative cache is namespaced by revision |
| `--health` fails | The container cannot read `LIBRARY_DIR` or its pointer |

#!/usr/bin/env bash
# Launch a Tauri dev instance with its own database, config and frontend port.
#
# The app already isolates the database, thumbnails, cache, face crops, models and
# Python venvs through FRANK_SHERLOCK_DATA_DIR. This wrapper adds the pieces that
# were still global — config.json, settings.toml, vision_calibration.json and the
# log files — plus a per-profile Vite port, so two libraries can run side by side.
#
# Usage:
#   bash scripts/dev-profile.sh <profile> [folder] [--port N] [-- tauri args]
#
# Examples:
#   # Personal photos: port 1420
#   bash scripts/dev-profile.sh photos ~/Pictures
#
#   # Animes: port 1421, its own database, config (and therefore publish folder) and logs
#   bash scripts/dev-profile.sh animes /mnt/nas/Anime
#
#   # Another profile, explicit port, folder chosen in the UI
#   bash scripts/dev-profile.sh scratch --port 1422
#
#   # Print the isolated environment and command without starting the app
#   bash scripts/dev-profile.sh animes /mnt/nas/Anime --dry-run
#
# Profile files live under $FRANK_SHERLOCK_PROFILE_ROOT (default:
# ~/.local/share/frank-sherlock-profiles/<profile>):
#   data/    -> FRANK_SHERLOCK_DATA_DIR   (index.sqlite, thumbnails, caches, models, venvs)
#   config/  -> FRANK_SHERLOCK_CONFIG_DIR (config.json, settings.toml, calibration)
#   logs/    -> FRANK_SHERLOCK_LOG_DIR    (the file the live-log modal reads)
#
# The models, venvs and caches are per data directory by design. If disk usage matters,
# symlink the shared, read-only parts after the first run:
#   ln -s <photos-profile>/data/models     <anime-profile>/data/models
#   ln -s <photos-profile>/data/surya_venv <anime-profile>/data/surya_venv
#   ln -s <photos-profile>/data/raw_venv   <anime-profile>/data/raw_venv

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$SCRIPT_DIR/../sherlock/desktop"

usage() {
    cat >&2 <<'EOF'
Usage: bash scripts/dev-profile.sh <profile> [folder] [--port N] [--dry-run] [-- tauri args]

Profiles:
  photos|fotos   default port 1420
  anime|animes   default port 1421
  other names    require an explicit --port

Examples:
  bash scripts/dev-profile.sh photos ~/Pictures
  bash scripts/dev-profile.sh animes /mnt/nas/Anime
  bash scripts/dev-profile.sh scratch --port 1422
  bash scripts/dev-profile.sh animes /mnt/nas/Anime -- --no-watch
EOF
}

profile=""
folder=""
port=""
dry_run=0
tauri_args=()

while (($#)); do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        --port)
            [[ $# -ge 2 ]] || {
                echo "dev-profile: --port needs a value" >&2
                exit 2
            }
            port="$2"
            shift 2
            ;;
        --port=*)
            port="${1#*=}"
            shift
            ;;
        --dry-run)
            dry_run=1
            shift
            ;;
        --)
            # Everything after `--` is forwarded to `tauri dev` as a flag.
            # Use the positional <folder> to pass the folder to the application.
            shift
            tauri_args+=("$@")
            break
            ;;
        -*)
            tauri_args+=("$1")
            shift
            ;;
        *)
            if [[ -z "$profile" ]]; then
                profile="$1"
            elif [[ -z "$folder" ]]; then
                folder="$1"
            else
                echo "dev-profile: unexpected argument '$1'" >&2
                exit 2
            fi
            shift
            ;;
    esac
done

if [[ -z "$profile" ]]; then
    usage >&2
    exit 2
fi
if [[ ! "$profile" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "dev-profile: profile must use letters, digits, dot, underscore or dash" >&2
    exit 2
fi

if [[ -z "$port" ]]; then
    case "$profile" in
        photos | fotos) port=1420 ;;
        anime | animes) port=1421 ;;
        *)
            echo "dev-profile: pass --port for profile '$profile' (photos=1420, animes=1421)" >&2
            exit 2
            ;;
    esac
fi
if [[ ! "$port" =~ ^[0-9]+$ ]] || ((port < 1 || port > 65535)); then
    echo "dev-profile: invalid port '$port'" >&2
    exit 2
fi

# VS Code installed as a snap exports GTK/GIO paths and SNAP_LIBRARY_PATH into its
# integrated terminal, which makes a native Tauri binary load snap libraries. Clear
# only the snap-owned values; legitimate NVIDIA entries are kept.
unset GTK_PATH GTK_EXE_PREFIX GTK_IM_MODULE_FILE \
    GDK_PIXBUF_MODULEDIR GDK_PIXBUF_MODULE_FILE \
    GIO_MODULE_DIR GSETTINGS_SCHEMA_DIR LOCPATH \
    SNAP_LIBRARY_PATH
if [[ "${LD_LIBRARY_PATH:-}" == *"/snap/"* ]]; then
    unset LD_LIBRARY_PATH
fi
if [[ "${LD_PRELOAD:-}" == *"/snap/"* ]]; then
    unset LD_PRELOAD
fi
export PATH="$HOME/.cargo/bin:$PATH"

profile_root="${FRANK_SHERLOCK_PROFILE_ROOT:-$HOME/.local/share/frank-sherlock-profiles/$profile}"
mkdir -p "$profile_root/data" "$profile_root/config" "$profile_root/logs"

export FRANK_SHERLOCK_DATA_DIR="$profile_root/data"
export FRANK_SHERLOCK_CONFIG_DIR="$profile_root/config"
export FRANK_SHERLOCK_LOG_DIR="$profile_root/logs"
export SHERLOCK_DEV_PORT="$port"

# The base tauri.conf.json pins build.devUrl to 1420. A per-profile overlay points it
# at this profile's Vite port; the base beforeDevCommand (`npm run dev`) picks the
# port up from SHERLOCK_DEV_PORT through vite.config.ts.
overlay="$profile_root/tauri.dev.json"
cat >"$overlay" <<EOF
{
  "build": {
    "devUrl": "http://localhost:$port"
  }
}
EOF

echo "dev-profile: profile=$profile port=$port"
echo "dev-profile: data=$FRANK_SHERLOCK_DATA_DIR"
echo "dev-profile: config=$FRANK_SHERLOCK_CONFIG_DIR"
echo "dev-profile: logs=$FRANK_SHERLOCK_LOG_DIR"
if [[ -n "$folder" ]]; then
    echo "dev-profile: folder=$folder"
fi

cd "$DESKTOP_DIR"

args=(--config "$overlay")
args+=("${tauri_args[@]}")
if [[ -n "$folder" ]]; then
    # `tauri dev -- [runnerArgs] -- [appArgs]`: the folder must reach argv[1] of the
    # application, which is what `get_cli_folder_path` reads.
    args+=(-- -- "$folder")
fi

if [[ "$dry_run" == 1 ]]; then
    echo "dev-profile: generated $overlay"
    printf 'dev-profile: command:'
    printf ' %q' npm run tauri:dev -- "${args[@]}"
    printf '\n'
    exit 0
fi

exec npm run tauri:dev -- "${args[@]}"

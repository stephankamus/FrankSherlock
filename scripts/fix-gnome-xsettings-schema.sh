#!/usr/bin/env bash
# Restore the font-rendering keys that gnome-settings-daemon 46 moved out of the
# `org.gnome.settings-daemon.plugins.xsettings` schema.
#
# Symptom (Ubuntu 24.04, i.e. every GTK3 app, Tauri included):
#
#   (sherlock_app:14925): GLib-GIO-ERROR **: Settings schema
#   'org.gnome.settings-daemon.plugins.xsettings' does not contain a key named
#   'antialiasing'
#
# Why: gnome-settings-daemon 46 kept `antialiasing`, `hinting` and `rgba-order`
# only in `org.gnome.settings-daemon.plugins.xsettings.deprecated`, but GDK's
# Wayland backend still reads them from the non-deprecated schema with an
# unguarded g_settings_get_enum() during gtk_init_check(). GLib aborts on the
# failed lookup, before any WebKit/application code runs, so updating
# libwebkit2gtk cannot help. GDK_BACKEND=x11 escapes it because the X11 backend
# takes these values from the XSETTINGS property instead.
#
# Fix: install a user-level copy of the schema, with the three keys added back,
# into $XDG_DATA_HOME/glib-2.0/schemas. That directory has higher priority than
# /usr/share/glib-2.0/schemas, so only this user is affected and the change
# survives package upgrades. The full upstream key set is copied, because a
# schema id defined twice resolves entirely to the highest-priority file.
#
# Usage:
#   bash scripts/fix-gnome-xsettings-schema.sh            # install / repair
#   bash scripts/fix-gnome-xsettings-schema.sh --dry-run  # what would change
#   bash scripts/fix-gnome-xsettings-schema.sh --revert   # remove the override
#
# Requires python3 and glib-compile-schemas (package libglib2.0-bin, installed
# by default on Ubuntu desktops).

set -euo pipefail

SCHEMA_ID="org.gnome.settings-daemon.plugins.xsettings"
SYSTEM_SCHEMA="/usr/share/glib-2.0/schemas/${SCHEMA_ID}.gschema.xml"
# glib-compile-schemas resolves `<key enum="...">` against the files in the same
# directory, so the gnome-settings-daemon enum table has to be copied too.
ENUMS_NAME="org.gnome.settings-daemon.enums.xml"
SYSTEM_ENUMS="/usr/share/glib-2.0/schemas/${ENUMS_NAME}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
DEST_DIR="$DATA_HOME/glib-2.0/schemas"
DEST="$DEST_DIR/${SCHEMA_ID}.gschema.xml"
DEST_ENUMS="$DEST_DIR/${ENUMS_NAME}"

dry_run=0
revert=0

for arg in "$@"; do
    case "$arg" in
        --dry-run) dry_run=1 ;;
        --revert) revert=1 ;;
        -h | --help)
            awk 'NR > 1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        *)
            echo "fix-gnome-xsettings-schema: unknown argument '$arg'" >&2
            exit 2
            ;;
    esac
done

have_key() {
    gsettings list-keys "$SCHEMA_ID" 2>/dev/null | grep -qx antialiasing
}

if [[ "$revert" == 1 ]]; then
    if [[ ! -e "$DEST" ]]; then
        echo "fix-gnome-xsettings-schema: nothing to revert ($DEST not present)"
        exit 0
    fi
    echo "fix-gnome-xsettings-schema: removing $DEST"
    [[ "$dry_run" == 1 ]] && exit 0
    rm -f "$DEST" "$DEST_ENUMS"
    glib-compile-schemas "$DEST_DIR"
    echo "fix-gnome-xsettings-schema: reverted"
    exit 0
fi

if have_key; then
    echo "fix-gnome-xsettings-schema: $SCHEMA_ID already has 'antialiasing', nothing to do"
    exit 0
fi

if [[ ! -f "$SYSTEM_SCHEMA" ]]; then
    echo "fix-gnome-xsettings-schema: $SYSTEM_SCHEMA not found" >&2
    echo "fix-gnome-xsettings-schema: this system does not ship gnome-settings-daemon schemas" >&2
    exit 1
fi

if ! command -v glib-compile-schemas >/dev/null 2>&1; then
    echo "fix-gnome-xsettings-schema: glib-compile-schemas not found" >&2
    echo "fix-gnome-xsettings-schema: install it with: sudo apt install libglib2.0-bin" >&2
    exit 1
fi

echo "fix-gnome-xsettings-schema: system schema = $SYSTEM_SCHEMA"
echo "fix-gnome-xsettings-schema: user override  = $DEST"

if [[ "$dry_run" == 1 ]]; then
    echo "fix-gnome-xsettings-schema: dry run, nothing written"
    exit 0
fi

mkdir -p "$DEST_DIR"
cp -f "$SYSTEM_ENUMS" "$DEST_ENUMS"

python3 - "$SYSTEM_SCHEMA" "$DEST" <<'PY'
import pathlib
import re
import sys

MISSING_KEYS = """\
    <key name="antialiasing" enum="org.gnome.settings-daemon.GsdFontAntialiasingMode">
      <default>'grayscale'</default>
      <summary>Antialiasing</summary>
      <description>Restored locally because gnome-settings-daemon 46 moved it to org.gnome.settings-daemon.plugins.xsettings.deprecated while WebKitGTK still reads it here.</description>
    </key>
    <key name="hinting" enum="org.gnome.settings-daemon.GsdFontHinting">
      <default>'slight'</default>
      <summary>Hinting</summary>
      <description>Restored locally because gnome-settings-daemon 46 moved it to org.gnome.settings-daemon.plugins.xsettings.deprecated while WebKitGTK still reads it here.</description>
    </key>
    <key name="rgba-order" enum="org.gnome.settings-daemon.GsdFontRgbaOrder">
      <default>'rgb'</default>
      <summary>RGBA order</summary>
      <description>Restored locally because gnome-settings-daemon 46 moved it to org.gnome.settings-daemon.plugins.xsettings.deprecated while WebKitGTK still reads it here.</description>
    </key>
"""

src_path, dst_path = sys.argv[1], sys.argv[2]
text = pathlib.Path(src_path).read_text(encoding="utf-8")

opening = re.compile(
    r'(<schema\b[^>]*\bid="org\.gnome\.settings-daemon\.plugins\.xsettings"[^>]*>\n)'
)
if not opening.search(text):
    sys.exit(f"could not find the {src_path} opening <schema> tag")

patched, count = opening.subn(lambda m: m.group(1) + MISSING_KEYS, text, count=1)
if count != 1:
    sys.exit("failed to patch the schema")

pathlib.Path(dst_path).write_text(patched, encoding="utf-8")
PY

glib-compile-schemas "$DEST_DIR"

if ! have_key; then
    echo "fix-gnome-xsettings-schema: override installed but 'antialiasing' is still missing" >&2
    echo "fix-gnome-xsettings-schema: check XDG_DATA_HOME ($DATA_HOME) for a shadowing schema dir" >&2
    exit 1
fi

echo "fix-gnome-xsettings-schema: installed; current values:"
for key in antialiasing hinting rgba-order; do
    printf '  %-12s %s\n' "$key" "$(gsettings get "$SCHEMA_ID" "$key")"
done

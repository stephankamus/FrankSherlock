import { convertFileSrc } from "@tauri-apps/api/core";
import {
  DESKTOP_CAPABILITIES,
  type FaceInfo,
  type LibraryClient,
  type PersonInfo,
  type RootInfo,
  type SearchItem,
} from "@frank-sherlock/shared";

import {
  findDuplicates,
  getVideoStreamUrl,
  listAlbums,
  listFacesForPerson,
  listPersons,
  listRoots,
  listSmartFolders,
  listSubdirectories,
  searchImages,
} from "./api";
import type { RootInfo as DesktopRootInfo } from "./types";

/**
 * `LibraryClient` implemented over Tauri.
 *
 * This is the whole platform-specific surface of the shared views: every
 * `invoke` and every `convertFileSrc` in a shared component is replaced by one
 * method here. The web client calls the same methods over HTTP, so a view never
 * needs to know which application mounted it.
 *
 * The paths stay where they are: a desktop row keeps `absPath`/`thumbnailPath`
 * and the asset protocol turns them into `asset://` URLs, which is exactly the
 * behaviour the components had before the client existed.
 */
function assetUrl(path: string | null | undefined): string | null {
  return path ? convertFileSrc(path) : null;
}

function toSharedRoot(root: DesktopRootInfo): RootInfo {
  return {
    id: root.id,
    name: root.rootName,
    path: root.rootPath,
    fileCount: root.fileCount,
    availability: root.availability,
    lastSeenAt: root.lastSeenAt ?? null,
    lastError: root.lastError ?? null,
  };
}

export function createTauriLibraryClient(): LibraryClient {
  return {
    capabilities: DESKTOP_CAPABILITIES,

    search(request) {
      // `includeHidden` is part of the desktop contract and travels as-is.
      return searchImages(request);
    },

    async listRoots() {
      return (await listRoots()).map(toSharedRoot);
    },

    listSubdirs(rootId, prefix) {
      return listSubdirectories(rootId, prefix);
    },

    listAlbums() {
      return listAlbums();
    },

    listSmartFolders() {
      return listSmartFolders();
    },

    async listPeople() {
      const persons = await listPersons([]);
      return persons.map(
        (person): PersonInfo => ({
          ...person,
          cropUrl: assetUrl(person.cropPath),
        }),
      );
    },

    async listFacesForPerson(personId) {
      const faces = await listFacesForPerson(personId);
      return faces.map(
        (face): FaceInfo => ({
          ...face,
          cropUrl: assetUrl(face.cropPath),
        }),
      );
    },

    listDuplicates(rootScope = []) {
      return findDuplicates(rootScope);
    },

    thumbUrl(item: SearchItem) {
      return assetUrl(item.thumbnailPath);
    },

    mediaUrl(item: SearchItem) {
      return assetUrl(item.absPath) ?? "";
    },

    previewUrl(item: SearchItem) {
      // Local files are already full resolution; the asset protocol is the reader.
      return assetUrl(item.absPath) ?? "";
    },

    async videoUrl(item: SearchItem) {
      if (!item.absPath) return "";
      try {
        // A local HTTP server with `Range`, because seeking over the asset protocol
        // is unreliable on Linux.
        return await getVideoStreamUrl(item.absPath);
      } catch {
        // macOS/Windows fall back to the asset protocol, as they always did.
        return convertFileSrc(item.absPath);
      }
    },

    faceCropUrl(face: FaceInfo) {
      return assetUrl(face.cropPath);
    },

    personCropUrl(person: PersonInfo) {
      return assetUrl(person.cropPath);
    },

    pdfUrl(item: SearchItem) {
      return assetUrl(item.absPath) ?? "";
    },
  };
}

/** Single instance mounted by `main.tsx`. */
export const tauriLibraryClient = createTauriLibraryClient();

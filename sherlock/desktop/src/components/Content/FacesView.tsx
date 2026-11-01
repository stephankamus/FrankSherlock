import {
  FacesView as SharedFacesView,
  LibraryProvider,
  type FacesActions,
} from "@frank-sherlock/shared";
import type { ComponentProps } from "react";

import {
  clusterFaces,
  getReclusterStatus,
  getUnclusteredFaceCount,
  hidePersonFiles,
  listIgnoredPersons,
  mergePersons,
  reassignFacesToPerson,
  reclusterFaces,
  renamePerson,
  setPersonsIgnored,
  setRepresentativeFace,
  unassignFaceFromPerson,
} from "../../api";
import { assetUrl, tauriLibraryClient } from "../../tauriLibraryClient";

/**
 * The desktop's faces screen: the shared gallery plus the write actions.
 *
 * `actions` is built at module scope so its identity never changes — the view
 * polls the re-cluster status while a run is active, and a new object on every
 * render would restart that interval. The provider is mounted here as well as in
 * `main.tsx` so a test can render this component on its own.
 */
const desktopFacesActions: FacesActions = {
  renamePerson,
  recluster: reclusterFaces,
  reclusterStatus: getReclusterStatus,
  unclusteredCount: getUnclusteredFaceCount,
  clusterNew: clusterFaces,
  unassignFace: unassignFaceFromPerson,
  reassignFaces: reassignFacesToPerson,
  setRepresentativeFace,
  mergePersons,
  async setPersonsIgnored(personIds, ignored) {
    await setPersonsIgnored(personIds, ignored);
  },
  async listIgnoredPeople() {
    const persons = await listIgnoredPersons([]);
    return persons.map((person) => ({ ...person, cropUrl: assetUrl(person.cropPath) }));
  },
  hidePhotos: hidePersonFiles,
};

type Props = Omit<ComponentProps<typeof SharedFacesView>, "actions">;

export default function FacesView(props: Props) {
  return (
    <LibraryProvider client={tauriLibraryClient}>
      <SharedFacesView {...props} actions={desktopFacesActions} />
    </LibraryProvider>
  );
}

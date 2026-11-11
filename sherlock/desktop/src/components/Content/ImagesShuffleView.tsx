import { SlideshowView } from "@frank-sherlock/shared";

type Props = {
  /** Portrait Mode interval, loaded from settings.toml. */
  intervalSeconds: number;
};

/**
 * Desktop host of the shared portrait frame.
 *
 * The view itself lives in `@frank-sherlock/shared` so the web deployment shows
 * exactly the same behaviour; this wrapper only passes the interval that comes
 * from the desktop settings.
 */
export default function ImagesShuffleView({ intervalSeconds }: Props) {
  return <SlideshowView intervalSeconds={intervalSeconds} />;
}

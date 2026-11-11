export type VersionArtwork = { banner: string; tile: string };

const images = import.meta.glob<string>("./assets/minecraft/*.jpg", {
  eager: true,
  import: "default",
  query: "?url",
});
const banner = images["./assets/minecraft/tricky-trials-banner.jpg"];
const tile = images["./assets/minecraft/tricky-trials-tile.jpg"];
const trickyTrialsArtwork = banner && tile ? { banner, tile } : undefined;

// Match update artwork explicitly: later 1.21.x drops have different artwork.
export function versionArtwork(version?: string): VersionArtwork | undefined {
  return version === "1.21" || version === "1.21.1" ? trickyTrialsArtwork : undefined;
}

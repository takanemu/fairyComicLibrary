import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";

export type TagCategory = "author" | "series" | "classification";

export const tagCategories: { value: TagCategory; label: string }[] = [
  { value: "author", label: "作者" },
  { value: "series", label: "シリーズ" },
  { value: "classification", label: "分類" },
];

export type Tag = { id: number; name: string; category: TagCategory };
export type TagWithCount = Tag & { archiveCount: number };

export type ArchiveSummary = {
  archiveId: string;
  filename: string;
  format: string;
  pageCount: number | null;
  fileSize: number;
  importedAt: string;
  lastReadAt: string | null;
  readCount: number;
  isFavorite: boolean;
  hasThumbnail: boolean;
  tags: Tag[];
};

export type ArchiveSortBy =
  | "fileSize"
  | "importedAt"
  | "lastReadAt"
  | "readCount"
  | "name";
export type SortDirection = "asc" | "desc";

const SORT_STORAGE_KEY = "fairyComicLibrary.sort";
const sortFields: ArchiveSortBy[] = [
  "fileSize",
  "importedAt",
  "lastReadAt",
  "readCount",
  "name",
];

export const defaultSort: { sortBy: ArchiveSortBy; sortDirection: SortDirection } = {
  sortBy: "importedAt",
  sortDirection: "desc",
};

export function loadSort(): typeof defaultSort {
  try {
    const saved = JSON.parse(localStorage.getItem(SORT_STORAGE_KEY) ?? "null");
    if (
      saved &&
      sortFields.includes(saved.sortBy) &&
      (saved.sortDirection === "asc" || saved.sortDirection === "desc")
    ) {
      return { sortBy: saved.sortBy, sortDirection: saved.sortDirection };
    }
  } catch {
    // 保存値が壊れていても既定の並びで起動する。
  }
  return defaultSort;
}

export function saveSort(sort: typeof defaultSort) {
  try {
    localStorage.setItem(SORT_STORAGE_KEY, JSON.stringify(sort));
  } catch {
    // 保存できなくても表示には影響させない。
  }
}

export type ArchiveQuery = {
  search: string | null;
  favoritesOnly: boolean;
  tagIds: number[];
  recentOnly: boolean;
  sortBy: ArchiveSortBy;
  sortDirection: SortDirection;
};

export const listArchives = (query: ArchiveQuery) =>
  invoke<ArchiveSummary[]>("list_archives", { query });

export const openInViewer = (archiveId: string) =>
invoke<void>("open_in_viewer", { archiveId });

export const getVideoDuration = (archiveId: string) =>
  invoke<number>("get_video_duration", { archiveId });

export function formatDuration(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainingSeconds = seconds % 60;
  const pad = (value: number) => value.toString().padStart(2, "0");
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(remainingSeconds)}`
    : `${minutes}:${pad(remainingSeconds)}`;
}

export const listTags = () => invoke<TagWithCount[]>("list_tags");

export const setFavorite = (archiveId: string, isFavorite: boolean) =>
  invoke<void>("set_favorite", { archiveId, isFavorite });

export const addTag = (archiveId: string, category: TagCategory, name: string) =>
  invoke<Tag>("add_tag", { archiveId, category, name });

export const addTagToArchives = (
  archiveIds: string[],
  category: TagCategory,
  name: string,
) => invoke<Tag>("add_tag_to_archives", { archiveIds, category, name });

export const removeTag = (archiveId: string, tagId: number) =>
  invoke<void>("remove_tag", { archiveId, tagId });

export const removeTagFromArchives = (archiveIds: string[], tagId: number) =>
  invoke<void>("remove_tag_from_archives", { archiveIds, tagId });

export type UnmanageSummary = {
  moved: number;
  failed: number;
  failures: string[];
  warnings: string[];
};

export const unmanageArchives = (archiveIds: string[]) =>
  invoke<UnmanageSummary>("unmanage_archives", { archiveIds });

// サムネイルは長期キャッシュされるため、再作成したIDにはクエリで版を付けて取り直させる。
const thumbnailRevisions = new Map<string, number>();
let thumbnailEpoch = 0;

export const thumbnailUrl = (archiveId: string) => {
  const revision = Math.max(thumbnailRevisions.get(archiveId) ?? 0, thumbnailEpoch);
  const url = convertFileSrc(archiveId, "thumbnail");
  return revision ? `${url}?v=${revision}` : url;
};

export const regenerateThumbnail = async (
  archiveId: string,
  entryName: string | null = null,
) => {
  await invoke<void>("regenerate_thumbnail", { archiveId, entryName });
  thumbnailRevisions.set(archiveId, Date.now());
};

export const setPackageImage = async (archiveId: string, imagePath: string) => {
  await invoke<void>("set_package_image", { archiveId, imagePath });
  thumbnailRevisions.set(archiveId, Date.now());
};

export type RegenerateSummary = {
  regenerated: number;
  failed: number;
  firstFailure: string | null;
};

// まとめて再作成した場合はどのIDが変わったか分からないため、全サムネイルのURLを更新する。
export const regenerateMissingThumbnails = async () => {
  try {
    return await invoke<RegenerateSummary>("regenerate_missing_thumbnails");
  } finally {
    thumbnailEpoch = Date.now();
  }
};

export const listCoverCandidates = (archiveId: string) =>
  invoke<string[]>("list_cover_candidates", { archiveId });

export function formatFileSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? value : value.toFixed(1)} ${units[unit]}`;
}

export type ConvertPhase = "analyze" | "convert" | "write" | "verify" | "finalize";

export type ConvertProgress = {
  phase: ConvertPhase;
  doneEntries: number;
  totalEntries: number;
  doneBytes: number;
  totalBytes: number;
  currentName: string | null;
  cancellable: boolean;
};

export type ConvertOutcome =
  | {
      status: "converted";
      archiveId: string;
      relativePath: string;
      originalSize: number;
      newSize: number;
      convertedImages: number;
      unchangedEntries: number;
      warning: string | null;
    }
  | { status: "duplicate"; archiveId: string; relativePath: string }
  | { status: "notSmaller"; originalSize: number; newSize: number }
  | { status: "cancelled" };

export const convertArchive = (
  archiveId: string,
  avifSpeed: number,
  onProgress: (progress: ConvertProgress) => void,
) => {
  const channel = new Channel<ConvertProgress>();
  channel.onmessage = onProgress;
  return invoke<ConvertOutcome>("convert_archive", {
    archiveId,
    avifSpeed,
    onProgress: channel,
  });
};

export const cancelConversion = () => invoke<void>("cancel_conversion");

export const isConvertible = (format: string) => format === "zip" || format === "cbz";
export const isEbook = (format: string) => format === "epub" || format === "azw3";
export const isVideo = (format: string) => format === "mkv" || format === "mp4";

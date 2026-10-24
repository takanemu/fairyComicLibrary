import { convertFileSrc, invoke } from "@tauri-apps/api/core";

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
  fileSize: number;
  importedAt: string;
  isFavorite: boolean;
  hasThumbnail: boolean;
  tags: Tag[];
};

export type ArchiveQuery = {
  search: string | null;
  favoritesOnly: boolean;
  tagIds: number[];
  sort: "name" | "recent";
};

export const listArchives = (query: ArchiveQuery) =>
  invoke<ArchiveSummary[]>("list_archives", { query });

export const listTags = () => invoke<TagWithCount[]>("list_tags");

export const setFavorite = (archiveId: string, isFavorite: boolean) =>
  invoke<void>("set_favorite", { archiveId, isFavorite });

export const addTag = (archiveId: string, category: TagCategory, name: string) =>
  invoke<Tag>("add_tag", { archiveId, category, name });

export const removeTag = (archiveId: string, tagId: number) =>
  invoke<void>("remove_tag", { archiveId, tagId });

export const thumbnailUrl = (archiveId: string) =>
  convertFileSrc(archiveId, "thumbnail");

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

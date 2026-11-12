import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ask, open } from "@tauri-apps/plugin-dialog";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import importButtonIcon from "./assets/book-import.png";
import { ConvertResult } from "./components/library/ConvertDialog";
import { LibraryView } from "./components/library/LibraryView";
import { TagTree } from "./components/library/TagTree";
import {
  ArchiveQuery,
  ArchiveSortBy,
  SortDirection,
  TagCategory,
  TagWithCount,
  TagGroup,
  createTagGroup,
  deleteTagGroup,
  listTags,
  listTagGroups,
  loadSort,
  moveTag,
  moveTagGroup,
  renameTagGroup,
  saveSort,
  cancelConversion,
  formatFileSize,
  regenerateMissingThumbnails,
  tagCategories,
} from "./lib/library";

type LibraryViewId = "all" | "favorites" | "recent";

const navigation: { id: LibraryViewId; label: string; icon: string }[] = [
  { id: "all", label: "すべての作品", icon: "▦" },
  { id: "favorites", label: "お気に入り", icon: "♡" },
  { id: "recent", label: "最近追加", icon: "◷" },
];

type RepositoryStatus = {
  path: string;
  schemaVersion: number;
  archiveCount: number;
  warnings: string[];
};

type ImportOutcome =
  | {
      status: "imported";
      archiveId: string;
      relativePath: string;
      fileSize: number;
      sourceRemoved: boolean;
      warning: string | null;
    }
  | {
      status: "duplicate";
      archiveId: string;
      relativePath: string;
    };

const videoExtensions = new Set(["mkv", "mp4"]);
const packageImageExtensions = new Set(["png", "webp", "jpg", "jpeg", "gif", "bmp"]);
const SIDEBAR_WIDTH_KEY = "fairyComicLibrary.sidebarWidth";

function loadSidebarWidth() {
  try {
    const saved = Number(localStorage.getItem(SIDEBAR_WIDTH_KEY));
    return Number.isFinite(saved) && saved >= 200 && saved <= 420 ? saved : 256;
  } catch {
    return 256;
  }
}

function saveSidebarWidth(width: number) {
  try {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, String(width));
  } catch {
    // 保存できなくても表示には影響させない。
  }
}

function extensionOf(path: string) {
  const filename = path.split("/").pop() ?? "";
  const dot = filename.lastIndexOf(".");
  return dot < 0 ? "" : filename.slice(dot + 1).toLowerCase();
}

function packageImageKey(path: string) {
  const separator = path.lastIndexOf("/");
  const filename = path.slice(separator + 1);
  const dot = filename.lastIndexOf(".");
  return `${path.slice(0, separator)}/${dot < 0 ? filename : filename.slice(0, dot)}`;
}

async function notify(title: string, body: string) {
  if (!("__TAURI_INTERNALS__" in window)) {
    return;
  }
  try {
    let granted = await isPermissionGranted();
    if (!granted) {
      granted = (await requestPermission()) === "granted";
    }
    if (granted) {
      sendNotification({ title, body });
    }
  } catch {
    // 通知はおまけの機能なので、失敗しても元の処理の結果には影響させない。
  }
}

async function notifyImportResult(
  imported: number,
  duplicate: number,
  failed: number,
  firstFailure: string | null,
) {
  const parts = [
    imported > 0 ? `${imported}件を登録` : null,
    duplicate > 0 ? `${duplicate}件は重複` : null,
    failed > 0 ? `${failed}件は失敗` : null,
  ].filter((part) => part !== null);
  if (parts.length === 0) {
    return;
  }
  await notify(
    failed > 0 ? "インポートが一部失敗しました" : "インポートが完了しました",
    [parts.join("、"), firstFailure].filter(Boolean).join("\n"),
  );
}

function App() {
  const [repository, setRepository] = useState<RepositoryStatus | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isSelecting, setIsSelecting] = useState(false);
  const [isImporting, setIsImporting] = useState(false);
  const [isBackingUp, setIsBackingUp] = useState(false);
  const [isRegenerating, setIsRegenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isDragging, setIsDragging] = useState(false);
  const [view, setView] = useState<LibraryViewId>("all");
  const [isMaximized, setIsMaximized] = useState(false);
  const [initialSort] = useState(loadSort);
  const [sortBy, setSortBy] = useState<ArchiveSortBy>(initialSort.sortBy);
  const [sortDirection, setSortDirection] = useState<SortDirection>(
    initialSort.sortDirection,
  );
  const [sidebarWidth, setSidebarWidth] = useState(loadSidebarWidth);
  const sidebarWidthRef = useRef(sidebarWidth);
  const sidebarElementRef = useRef<HTMLElement | null>(null);
  const sidebarSeparatorRef = useRef<HTMLDivElement | null>(null);
  const isResizingSidebarRef = useRef(false);
  useEffect(() => {
    saveSort({ sortBy, sortDirection });
  }, [sortBy, sortDirection]);
  const [searchText, setSearchText] = useState("");
  const [archiveCount, setArchiveCount] = useState<number | null>(null);
  const [search, setSearch] = useState("");
  const [tagIds, setTagIds] = useState<number[]>([]);
  const [tags, setTags] = useState<TagWithCount[]>([]);
  const [tagGroups, setTagGroups] = useState<TagGroup[]>([]);
  const [expandedTagCategories, setExpandedTagCategories] = useState<
    Record<TagCategory, boolean>
  >({ author: true, series: true, classification: true });
  const [version, setVersion] = useState(0);
  const [isConverting, setIsConverting] = useState(false);
  const importingRef = useRef(false);
  const convertingRef = useRef(false);
  const closeAfterConvertRef = useRef(false);
  const canImportRef = useRef(false);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) {
      return;
    }

    const appWindow = getCurrentWindow();
    let active = true;
    let unlisten: (() => void) | undefined;

    const updateMaximizedState = async () => {
      try {
        const maximized = await appWindow.isMaximized();
        if (active) {
          setIsMaximized(maximized);
        }
      } catch (reason) {
        if (active) {
          setError(`ウィンドウ状態を確認できませんでした: ${String(reason)}`);
        }
      }
    };

    void appWindow
      .onResized(() => {
        void updateMaximizedState();
      })
      .then((stopListening) => {
        if (active) {
          unlisten = stopListening;
          void updateMaximizedState();
        } else {
          stopListening();
        }
      })
      .catch((reason: unknown) => {
        if (active) {
          setError(`ウィンドウ状態の監視を開始できませんでした: ${String(reason)}`);
        }
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let isMounted = true;

    invoke<RepositoryStatus | null>("get_repository_status")
      .then((status) => {
        if (isMounted) {
          setRepository(status);
          setError(null);
        }
      })
      .catch((reason: unknown) => {
        if (isMounted) {
          setError(
            `保存済みリポジトリを開けませんでした: ${String(reason)}`,
          );
        }
      })
      .finally(() => {
        if (isMounted) {
          setIsLoading(false);
        }
      });

    return () => {
      isMounted = false;
    };
  }, []);

  async function selectRepository() {
    setIsSelecting(true);
    setError(null);

    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "アプリ管理リポジトリを選択",
      });
      if (typeof selected !== "string") {
        return;
      }

      const status = await invoke<RepositoryStatus>("open_repository", {
        path: selected,
      });
      setRepository(status);
    } catch (reason) {
      setError(`リポジトリを開けませんでした: ${String(reason)}`);
    } finally {
      setIsSelecting(false);
    }
  }

  canImportRef.current = repository !== null;

  async function regenerateMissing() {
    setError(null);
    setIsRegenerating(true);
    try {
      const summary = await regenerateMissingThumbnails();
      const status = await invoke<RepositoryStatus | null>("get_repository_status");
      setRepository(status);
      setVersion((current) => current + 1);
      void notify(
        summary.failed > 0
          ? "サムネイルの再作成が一部失敗しました"
          : "サムネイルを再作成しました",
        [
          `${summary.regenerated}件を再作成${summary.failed > 0 ? `、${summary.failed}件は失敗` : ""}`,
          summary.firstFailure,
        ]
          .filter(Boolean)
          .join("\n"),
      );
    } catch (reason) {
      setError(`サムネイルを再作成できませんでした: ${String(reason)}`);
    } finally {
      setIsRegenerating(false);
    }
  }

  async function backupRepository() {
    setError(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "バックアップ先（空のディレクトリ）を選択",
      });
      if (typeof selected !== "string") {
        return;
      }
      setIsBackingUp(true);
      const summary = await invoke<{ archiveCount: number; fileCount: number }>(
        "backup_repository",
        { destination: selected },
      );
      void notify(
        "バックアップが完了しました",
        `${summary.archiveCount}件のアーカイブをコピーしました`,
      );
    } catch (reason) {
      setError(`バックアップできませんでした: ${String(reason)}`);
    } finally {
      setIsBackingUp(false);
    }
  }

  const handleConvertingChange = useCallback((busy: boolean) => {
    convertingRef.current = busy;
    setIsConverting(busy);
    if (!busy && closeAfterConvertRef.current) {
      void getCurrentWindow().destroy();
    }
  }, []);

  const handleConvertComplete = useCallback((filename: string, result: ConvertResult) => {
    if (result.status === "converted") {
      void notify(
        "7z + AVIF への変換が完了しました",
        `${filename}\n${formatFileSize(result.originalSize)} → ${formatFileSize(result.newSize)}`,
      );
    } else if (result.status === "failed") {
      void notify("変換に失敗しました", `${filename}\n${result.message}`);
    } else if (result.status === "notSmaller") {
      void notify("変換を取りやめました", `${filename}\n変換後のほうが小さくならなかったため、元を残しました`);
    }
  }, []);

  // 変換中にウィンドウを閉じる場合は、確認のうえ中断し、元のアーカイブを保ったまま終了する。
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) {
      return;
    }
    let unlisten: (() => void) | undefined;
    let isMounted = true;
    getCurrentWindow()
      .onCloseRequested(async (event) => {
        if (!convertingRef.current) {
          return;
        }
        event.preventDefault();
        const confirmed = await ask(
          "7z + AVIF への変換を中断して終了しますか？元のアーカイブはそのまま残ります。",
          { title: "変換中です", kind: "warning" },
        );
        if (confirmed) {
          closeAfterConvertRef.current = true;
          await cancelConversion().catch(() => undefined);
        }
      })
      .then((dispose) => {
        if (isMounted) {
          unlisten = dispose;
        } else {
          dispose();
        }
      })
      .catch(() => undefined);
    return () => {
      isMounted = false;
      unlisten?.();
    };
  }, []);

  const bumpVersion = useCallback(() => setVersion((current) => current + 1), []);

  async function createGroup(category: TagCategory, name: string, parentId: number | null) {
    setError(null);
    try {
      await createTagGroup(category, name, parentId);
      bumpVersion();
    } catch (reason) {
      setError(`グループを作成できませんでした: ${String(reason)}`);
    }
  }

  async function renameGroup(groupId: number, name: string) {
    setError(null);
    try {
      await renameTagGroup(groupId, name);
      bumpVersion();
    } catch (reason) {
      setError(`グループを更新できませんでした: ${String(reason)}`);
    }
  }

  async function removeGroup(groupId: number) {
    setError(null);
    try {
      await deleteTagGroup(groupId);
      bumpVersion();
    } catch (reason) {
      setError(`グループを削除できませんでした: ${String(reason)}`);
    }
  }

  async function relocateGroup(groupId: number, parentId: number | null) {
    setError(null);
    try {
      await moveTagGroup(groupId, parentId);
      bumpVersion();
    } catch (reason) {
      setError(`グループを移動できませんでした: ${String(reason)}`);
    }
  }

  async function updateTagParent(tagId: number, parentId: number | null) {
    setError(null);
    try {
      await moveTag(tagId, parentId);
      bumpVersion();
    } catch (reason) {
      setError(`タグを移動できませんでした: ${String(reason)}`);
    }
  }

  useEffect(() => {
    const timer = window.setTimeout(() => setSearch(searchText), 250);
    return () => window.clearTimeout(timer);
  }, [searchText]);

  const hasRepository = repository !== null;
  useEffect(() => {
    if (!hasRepository) {
      setTags([]);
      setTagGroups([]);
      return;
    }
    let cancelled = false;
    Promise.all([listTags(), listTagGroups()])
      .then(([result, groupResult]) => {
        if (cancelled) {
          return;
        }
        setTags(result);
        setTagGroups(groupResult);
        setTagIds((current) => {
          const remaining = current.filter((id) =>
            result.some((tag) => tag.id === id),
          );
          return remaining.length === current.length ? current : remaining;
        });
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          setError(`タグ一覧を読み込めませんでした: ${String(reason)}`);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [hasRepository, version]);

  const query = useMemo<ArchiveQuery>(
    () => ({
      search: search.trim() || null,
      favoritesOnly: view === "favorites",
      tagIds,
      recentOnly: view === "recent",
      sortBy,
      sortDirection,
    }),
    [search, sortBy, sortDirection, view, tagIds],
  );
  const isFiltered =
    query.search !== null || tagIds.length > 0 || view === "favorites";

  const importPaths = useCallback(async (paths: string[]) => {
    if (importingRef.current || convertingRef.current || paths.length === 0) {
      return;
    }
    importingRef.current = true;
    setError(null);
    setIsImporting(true);

    try {
      let importedCount = 0;
      let duplicateCount = 0;
      let failedCount = 0;
      let firstFailure: string | null = null;
      const candidates = new Map<string, string[]>();
      const videosByKey = new Map<string, number>();
      for (const path of paths) {
        const extension = extensionOf(path);
        const key = packageImageKey(path);
        if (videoExtensions.has(extension)) {
          videosByKey.set(key, (videosByKey.get(key) ?? 0) + 1);
        } else if (packageImageExtensions.has(extension)) {
          candidates.set(key, [...(candidates.get(key) ?? []), path]);
        }
      }
      const matchedImages = new Set<string>();
      const packageImageByVideo = new Map<string, string>();
      for (const path of paths) {
        if (!videoExtensions.has(extensionOf(path))) continue;
        const key = packageImageKey(path);
        const images = candidates.get(key) ?? [];
        if (images.length > 0) {
          images.forEach((image) => matchedImages.add(image));
        }
        if (images.length === 1 && videosByKey.get(key) === 1) {
          packageImageByVideo.set(path, images[0]);
        }
      }

      for (const path of paths) {
        if (matchedImages.has(path)) continue;
        try {
          const outcome = await invoke<ImportOutcome>("import_archive", {
            path,
            thumbnailPath: packageImageByVideo.get(path) ?? null,
          });
          if (outcome.status === "duplicate") {
            duplicateCount += 1;
          } else {
            importedCount += 1;
          }
        } catch (reason) {
          failedCount += 1;
          firstFailure ??= `${path.split("/").pop()}: ${String(reason)}`;
        }
      }

      void notifyImportResult(importedCount, duplicateCount, failedCount, firstFailure);
      if (importedCount > 0) {
        setRepository((current) =>
          current
            ? { ...current, archiveCount: current.archiveCount + importedCount }
            : current,
        );
        setVersion((current) => current + 1);
      }
    } finally {
      importingRef.current = false;
      setIsImporting(false);
    }
  }, []);

  useEffect(() => {
    let isMounted = true;
    let unlisten: (() => void) | undefined;

    if (!("__TAURI_INTERNALS__" in window)) {
      return;
    }

    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          setIsDragging(
            canImportRef.current && !importingRef.current && !convertingRef.current,
          );
        } else if (payload.type === "leave") {
          setIsDragging(false);
        } else if (payload.type === "drop") {
          setIsDragging(false);
          if (!canImportRef.current) {
            setError("先にアプリ管理リポジトリを設定してください。");
          } else {
            void importPaths(payload.paths);
          }
        }
      })
      .then((dispose) => {
        if (isMounted) {
          unlisten = dispose;
        } else {
          dispose();
        }
      })
      .catch((reason: unknown) => {
        if (isMounted) {
          setError(`ドラッグ＆ドロップを有効にできませんでした: ${String(reason)}`);
        }
      });

    return () => {
      isMounted = false;
      unlisten?.();
    };
  }, [importPaths]);

  async function importArchives() {
    try {
      const selected = await open({
        directory: false,
        multiple: true,
        title: "インポートするアーカイブを選択",
        filters: [
          {
            name: "作品とパッケージ画像",
            extensions: [
              "7z", "zip", "rar", "cbz", "cbr", "pdf", "epub", "azw3",
              "mkv", "mp4", "png", "webp", "jpg", "jpeg", "gif", "bmp",
            ],
          },
        ],
      });
      if (!selected) {
        return;
      }
      await importPaths(Array.isArray(selected) ? selected : [selected]);
    } catch (reason) {
      setError(`アーカイブを選択できませんでした: ${String(reason)}`);
    }
  }

  function toggleTagGroup(ids: number[]) {
    if (ids.length === 0) return;
    setTagIds((current) => {
      const selectedAll = ids.every((id) => current.includes(id));
      return selectedAll
        ? current.filter((id) => !ids.includes(id))
        : [...new Set([...current, ...ids])];
    });
  }

  function finishSidebarResize() {
    if (!isResizingSidebarRef.current) return;
    isResizingSidebarRef.current = false;
    setSidebarWidth(sidebarWidthRef.current);
    saveSidebarWidth(sidebarWidthRef.current);
    if (sidebarSeparatorRef.current) {
      sidebarSeparatorRef.current.style.backgroundColor = "";
    }
  }

  return (
    <div className="relative flex h-screen overflow-hidden bg-canvas text-slate-100">
      {isDragging && (
        <div
          aria-live="polite"
          className="pointer-events-none absolute inset-3 z-50 grid place-items-center rounded-2xl border-2 border-dashed border-violet-300/60 bg-violet-950/70 backdrop-blur-sm"
        >
          <p className="text-lg font-medium text-violet-100">
            ここにドロップしてインポート
          </p>
        </div>
      )}
      <aside
        ref={sidebarElementRef}
        className="flex h-full shrink-0 flex-col border-r border-white/5 bg-sidebar px-3 py-6 sm:px-5"
        style={{ width: sidebarWidth }}
      >
        <nav aria-label="メインナビゲーション" className="space-y-1">
          {navigation.map((item) => (
            <button
              aria-current={item.id === view ? "page" : undefined}
              className={`flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left text-sm transition ${
                item.id === view
                  ? "bg-white/[0.08] text-white"
                  : "text-slate-400 hover:bg-white/[0.04] hover:text-slate-200"
              }`}
              key={item.id}
              onClick={() => setView(item.id)}
              type="button"
            >
              <span aria-hidden="true" className="w-5 text-center text-base">
                {item.icon}
              </span>
              {item.label}
            </button>
          ))}
        </nav>

        {repository && (
          <div className="mt-6 min-h-0 flex-1 overflow-y-auto overscroll-contain pr-1">
            {tagCategories.map((category) => {
              const items = tags.filter((tag) => tag.category === category.value);
              const groups = tagGroups.filter(
                (group) => group.category === category.value,
              );
              const expanded = expandedTagCategories[category.value];
              const selectedCount = items.filter((tag) => tagIds.includes(tag.id)).length;
              const categoryId = `tag-category-${category.value}`;
              return (
                <div className="mb-4" key={category.value}>
                  <button
                    aria-controls={categoryId}
                    aria-expanded={expanded}
                    className="mb-1 flex w-full items-center justify-between rounded-lg px-3 py-2 text-left text-[10px] font-semibold uppercase tracking-[0.18em] text-slate-500 transition hover:bg-white/[0.04] hover:text-slate-300"
                    onClick={() =>
                      setExpandedTagCategories((current) => ({
                        ...current,
                        [category.value]: !current[category.value],
                      }))
                    }
                    type="button"
                  >
                    <span className="flex items-center gap-2">
                      <span aria-hidden="true">{expanded ? "⌄" : "›"}</span>
                      {category.label}
                      <span className="text-[10px] tracking-normal text-slate-600">
                        {selectedCount > 0 ? `${selectedCount}/` : ""}
                        {items.length} タグ
                      </span>
                    </span>
                  </button>
                  <div
                    hidden={!expanded}
                    id={categoryId}
                  >
                    <TagTree
                      category={category.value}
                      groups={groups}
                      onCreateGroup={(name, parentId) =>
                        void createGroup(category.value, name, parentId)
                      }
                      onDeleteGroup={(id) => void removeGroup(id)}
                      onMoveGroup={(id, parentId) =>
                        void relocateGroup(id, parentId)
                      }
                      onMoveTag={(id, parentId) =>
                        void updateTagParent(id, parentId)
                      }
                      onRenameGroup={(id, name) =>
                        void renameGroup(id, name)
                      }
                      onToggleGroup={toggleTagGroup}
                      onToggleTag={(id) =>
                        setTagIds((current) =>
                          current.includes(id)
                            ? current.filter((value) => value !== id)
                            : [...current, id],
                        )
                      }
                      selectedTagIds={tagIds}
                      tags={items}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        )}

        <div className="mt-auto shrink-0 rounded-xl border border-white/[0.06] bg-white/[0.025] p-4">
          <p className="text-xs font-medium text-slate-300">アプリ管理リポジトリ</p>
          {isLoading ? (
            <p className="mt-2 text-xs text-slate-500">状態を確認しています…</p>
          ) : repository ? null : (
            <p className="mt-2 text-xs leading-5 text-slate-500">
              リポジトリを選択または新規作成してください。
            </p>
          )}
          <button
            className={`mt-4 w-full rounded-lg border px-3 py-2 text-xs font-medium transition disabled:cursor-wait disabled:opacity-50 ${
              repository
                ? "border-white/[0.08] text-white hover:bg-white/[0.05]"
                : "border-rose-400/40 text-rose-400 hover:bg-rose-400/10"
            }`}
            disabled={isSelecting}
            onClick={selectRepository}
            type="button"
          >
            {isSelecting ? "処理中…" : repository ? "場所を変更" : "未登録"}
          </button>
          {repository && (
            <button
              className="mt-2 w-full rounded-lg border border-white/[0.08] px-3 py-2 text-xs font-medium text-white transition hover:bg-white/[0.05] disabled:cursor-wait disabled:opacity-50"
              disabled={isBackingUp || isImporting || isConverting}
              onClick={backupRepository}
              title="リポジトリ全体を、別PCへ移せる形でコピーします"
              type="button"
            >
              {isBackingUp ? "バックアップ中…" : "バックアップ"}
            </button>
          )}
        </div>
      </aside>
      <div
        aria-label="サイドバーの幅を変更"
        aria-orientation="vertical"
        aria-valuemax={420}
        aria-valuemin={200}
        aria-valuenow={sidebarWidth}
        className="z-10 w-1 shrink-0 cursor-col-resize touch-none bg-white/[0.03] transition-colors hover:bg-violet-300/40"
        onKeyDown={(event) => {
          if (event.key === "ArrowLeft") {
            const width = Math.max(200, sidebarWidthRef.current - 16);
            sidebarWidthRef.current = width;
            setSidebarWidth(width);
            if (sidebarElementRef.current) {
              sidebarElementRef.current.style.width = `${width}px`;
            }
            event.currentTarget.setAttribute("aria-valuenow", String(width));
            saveSidebarWidth(width);
          } else if (event.key === "ArrowRight") {
            const width = Math.min(420, sidebarWidthRef.current + 16);
            sidebarWidthRef.current = width;
            setSidebarWidth(width);
            if (sidebarElementRef.current) {
              sidebarElementRef.current.style.width = `${width}px`;
            }
            event.currentTarget.setAttribute("aria-valuenow", String(width));
            saveSidebarWidth(width);
          }
        }}
        ref={sidebarSeparatorRef}
        onPointerDown={(event) => {
          event.preventDefault();
          event.currentTarget.setPointerCapture(event.pointerId);
          isResizingSidebarRef.current = true;
          event.currentTarget.style.backgroundColor = "rgb(196 181 253 / 0.5)";
        }}
        onPointerMove={(event) => {
          if (!isResizingSidebarRef.current) return;
          const maximum = Math.min(420, window.innerWidth - 320);
          const width = Math.max(200, Math.min(maximum, event.clientX));
          sidebarWidthRef.current = width;
          if (sidebarElementRef.current) {
            sidebarElementRef.current.style.width = `${width}px`;
          }
          event.currentTarget.setAttribute("aria-valuenow", String(width));
        }}
        onPointerUp={finishSidebarResize}
        onPointerCancel={finishSidebarResize}
        onLostPointerCapture={finishSidebarResize}
        role="separator"
        tabIndex={0}
        title="ドラッグしてサイドバーの幅を変更"
      />
      <main className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header
          className="flex h-12 shrink-0 cursor-move select-none items-center justify-between border-b border-white/5 pl-10 pr-3"
          onMouseDown={(event) => {
            if (
              event.button !== 0 ||
              (event.target instanceof Element && event.target.closest("button")) ||
              !("__TAURI_INTERNALS__" in window)
            ) {
              return;
            }
            void getCurrentWindow()
              .startDragging()
              .catch((reason: unknown) =>
                setError(`ウィンドウを移動できませんでした: ${String(reason)}`),
              );
          }}
        >
          <div className="ml-auto flex items-center gap-2">
            <button
              aria-label={isMaximized ? "元に戻す" : "最大化"}
              className="grid size-8 cursor-pointer place-items-center rounded-lg text-slate-400 transition hover:bg-white/[0.08] hover:text-white"
              onClick={() => {
                if (!("__TAURI_INTERNALS__" in window)) return;
                const appWindow = getCurrentWindow();
                void appWindow
                  .toggleMaximize()
                  .then(() => appWindow.isMaximized())
                  .then(setIsMaximized)
                  .catch((reason: unknown) => {
                    setError(`ウィンドウを最大化できませんでした: ${String(reason)}`);
                  });
              }}
              title={isMaximized ? "元に戻す" : "最大化"}
              type="button"
            >
              <svg aria-hidden="true" fill="none" height="14" viewBox="0 0 14 14" width="14">
                {isMaximized ? (
                  <>
                    <path d="M5.5 4.5v-2h6v6h-2" stroke="currentColor" strokeLinejoin="round" strokeWidth="1.3" />
                    <rect height="6" rx="0.7" stroke="currentColor" strokeWidth="1.3" width="6" x="2.5" y="5.5" />
                  </>
                ) : (
                  <rect height="9" rx="0.8" stroke="currentColor" strokeWidth="1.3" width="9" x="2.5" y="2.5" />
                )}
              </svg>
            </button>
            <button
              aria-label="閉じる"
              className="grid size-8 cursor-pointer place-items-center rounded-lg text-slate-400 transition hover:bg-rose-400/20 hover:text-rose-200"
              onClick={() => {
                if (!("__TAURI_INTERNALS__" in window)) return;
                void getCurrentWindow()
                  .close()
                  .catch((reason: unknown) =>
                    setError(`ウィンドウを閉じられませんでした: ${String(reason)}`),
                  );
              }}
              title="閉じる"
              type="button"
            >
              <svg aria-hidden="true" fill="none" height="14" viewBox="0 0 14 14" width="14">
                <path d="m3 3 8 8M11 3l-8 8" stroke="currentColor" strokeLinecap="round" strokeWidth="1.5" />
              </svg>
            </button>
          </div>
        </header>

        <section className="flex min-h-0 flex-1 flex-col px-6 pb-6">
          {repository && (
            <div className="flex h-14 shrink-0 items-center justify-between gap-4">
              <p className="shrink-0 text-xs text-slate-500">
                {archiveCount !== null ? `${archiveCount} 件` : ""}
              </p>
              <div className="flex items-center gap-2">
                <label className="text-xs text-slate-300" htmlFor="archive-sort-by">
                  並び替え
                </label>
                <div className="relative">
                  <select
                    className="w-full appearance-none rounded-lg border border-white/[0.07] bg-slate-800 py-2 pl-3 pr-7 text-xs text-slate-100 outline-none transition focus:border-violet-300/50"
                    id="archive-sort-by"
                    onChange={(event) => setSortBy(event.target.value as ArchiveSortBy)}
                    value={sortBy}
                  >
                    <option value="fileSize">ファイルサイズ</option>
                    <option value="importedAt">登録日時</option>
                    <option value="lastReadAt">最終読書日時</option>
                    <option value="readCount">読書回数</option>
                    <option value="name">ファイル名称</option>
                  </select>
                  <span aria-hidden="true" className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 text-[10px] text-slate-300">▾</span>
                </div>
                <div className="relative">
                  <select
                    aria-label="並び順"
                    className="w-full appearance-none rounded-lg border border-white/[0.07] bg-slate-800 py-2 pl-3 pr-7 text-xs text-slate-100 outline-none transition focus:border-violet-300/50"
                    onChange={(event) => setSortDirection(event.target.value as SortDirection)}
                    value={sortDirection}
                  >
                    <option value="asc">昇順</option>
                    <option value="desc">降順</option>
                  </select>
                  <span aria-hidden="true" className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 text-[10px] text-slate-300">▾</span>
                </div>
                <label className="flex items-center gap-2 rounded-lg border border-white/[0.07] bg-white/[0.025] px-3 py-2 text-xs text-slate-300">
                  <span aria-hidden="true" className="text-slate-500">
                    ⌕
                  </span>
                  <input
                    aria-label="ファイル名・タグで検索"
                    className="w-32 bg-transparent outline-none placeholder:text-slate-600 xl:w-56"
                    onChange={(event) => setSearchText(event.target.value)}
                    placeholder="ファイル名・タグで検索"
                    type="search"
                    value={searchText}
                  />
                </label>
              </div>
            </div>
          )}

          <div className="flex min-h-0 flex-1 flex-col overflow-y-auto overscroll-contain">
            {repository && repository.warnings.length > 0 && (
              <div
                className="mb-5 space-y-1 rounded-xl border border-amber-300/25 bg-amber-300/[0.06] px-4 py-3 text-xs leading-5 text-amber-200"
                role="alert"
              >
                <p className="font-medium">リポジトリの検証で問題が見つかりました</p>
                {repository.warnings.map((warning) => (
                  <p className="break-all" key={warning}>
                    {warning}
                  </p>
                ))}
                {repository.warnings.some((warning) => warning.startsWith("サムネイル")) && (
                  <button
                    className="mt-2 rounded-lg border border-amber-300/30 px-3 py-1.5 text-xs font-medium text-amber-100 transition hover:bg-amber-300/10 disabled:opacity-50"
                    disabled={isRegenerating}
                    onClick={regenerateMissing}
                    type="button"
                  >
                    {isRegenerating ? "再作成中…" : "欠けたサムネイルを再作成"}
                  </button>
                )}
              </div>
            )}

            {error && (
              <div
                className="mb-5 rounded-xl border border-rose-300/20 bg-rose-300/[0.06] px-4 py-3 text-sm text-rose-200"
                role="alert"
              >
                {error}
              </div>
            )}

            {repository ? (
              <LibraryView
                isFiltered={isFiltered}
                onConvertComplete={handleConvertComplete}
                onConvertingChange={handleConvertingChange}
                onCountChange={setArchiveCount}
                onError={setError}
                onMutated={bumpVersion}
                activeTagIds={tagIds}
                onToggleTagFilter={(id) =>
                  setTagIds((current) =>
                    current.includes(id)
                      ? current.filter((value) => value !== id)
                      : [...current, id],
                  )
                }
                query={query}
                version={version}
              />
            ) : (
              <div className="flex flex-1 flex-col items-center justify-center rounded-2xl border border-dashed border-white/[0.09] bg-white/[0.012] px-6 py-16 text-center">
                <div className="mb-6 grid size-[68px] place-items-center rounded-2xl border border-violet-300/10 bg-violet-300/[0.06] text-3xl text-violet-200/80">
                  ◫
                </div>
                <h3 className="text-base font-medium text-slate-200">
                  リポジトリを設定してください
                </h3>
                <p className="mt-2 max-w-sm text-sm leading-6 text-slate-500">
                  空のディレクトリを選ぶと、アーカイブ・サムネイル・SQLiteデータベース用の構成を作成します。フォルダーは事前に作成してください。
                </p>
                <button
                  className="mt-7 rounded-lg bg-violet-400/15 px-4 py-2.5 text-xs font-medium text-violet-200 transition hover:bg-violet-400/20 disabled:cursor-wait disabled:opacity-50"
                  disabled={isSelecting || isLoading}
                  onClick={selectRepository}
                  type="button"
                >
                  {isSelecting ? "準備中…" : "リポジトリを選択"}
                </button>
              </div>
            )}
          </div>
        </section>
      </main>
      {repository && (
        <button
          aria-label={isImporting ? "インポート中" : "アーカイブをインポート"}
          className="fixed bottom-6 right-6 z-50 size-14 rounded-2xl transition duration-200 drop-shadow-[0_10px_8px_rgba(0,0,0,0.45)] hover:-translate-y-0.5 hover:drop-shadow-[0_14px_10px_rgba(0,0,0,0.5)] active:translate-y-0 active:drop-shadow-[0_4px_4px_rgba(0,0,0,0.45)] disabled:cursor-wait disabled:opacity-60 disabled:hover:translate-y-0"
          disabled={isImporting || isConverting}
          onClick={importArchives}
          title={isImporting ? "インポート中…" : "アーカイブをインポート（ウィンドウへのドロップも可）"}
          type="button"
        >
          <img
            alt=""
            className={`size-full ${isImporting ? "animate-pulse" : ""}`}
            draggable={false}
            src={importButtonIcon}
          />
        </button>
      )}
    </div>
  );
}

export default App;

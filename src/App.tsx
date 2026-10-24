import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { LibraryView } from "./components/library/LibraryView";
import {
  ArchiveQuery,
  TagWithCount,
  listTags,
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

function App() {
  const [repository, setRepository] = useState<RepositoryStatus | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isSelecting, setIsSelecting] = useState(false);
  const [isImporting, setIsImporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [importMessages, setImportMessages] = useState<string[]>([]);
  const [isDragging, setIsDragging] = useState(false);
  const [view, setView] = useState<LibraryViewId>("all");
  const [searchText, setSearchText] = useState("");
  const [search, setSearch] = useState("");
  const [tagIds, setTagIds] = useState<number[]>([]);
  const [tags, setTags] = useState<TagWithCount[]>([]);
  const [version, setVersion] = useState(0);
  const importingRef = useRef(false);
  const canImportRef = useRef(false);

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

  const bumpVersion = useCallback(() => setVersion((current) => current + 1), []);

  useEffect(() => {
    const timer = window.setTimeout(() => setSearch(searchText), 250);
    return () => window.clearTimeout(timer);
  }, [searchText]);

  const hasRepository = repository !== null;
  useEffect(() => {
    if (!hasRepository) {
      setTags([]);
      return;
    }
    let cancelled = false;
    listTags()
      .then((result) => {
        if (cancelled) {
          return;
        }
        setTags(result);
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
      sort: view === "recent" ? "recent" : "name",
    }),
    [search, view, tagIds],
  );
  const isFiltered =
    query.search !== null || tagIds.length > 0 || view === "favorites";

  const importPaths = useCallback(async (paths: string[]) => {
    if (importingRef.current || paths.length === 0) {
      return;
    }
    importingRef.current = true;
    setError(null);
    setImportMessages([]);
    setIsImporting(true);

    try {
      const messages: string[] = [];
      let importedCount = 0;

      for (const path of paths) {
        try {
          const outcome = await invoke<ImportOutcome>("import_archive", {
            path,
          });
          if (outcome.status === "duplicate") {
            messages.push(
              `重複をスキップ: ${path}（登録先: ${outcome.relativePath}）`,
            );
            continue;
          }

          importedCount += 1;
          messages.push(`インポート完了: ${outcome.relativePath}`);
          if (outcome.warning) {
            messages.push(outcome.warning);
          }
        } catch (reason) {
          messages.push(`失敗: ${path} — ${String(reason)}`);
        }
      }

      setImportMessages(messages);
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
          setIsDragging(canImportRef.current && !importingRef.current);
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
            name: "アーカイブ",
            extensions: ["7z", "zip", "rar", "cbz", "cbr"],
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

  return (
    <div className="relative flex min-h-screen bg-canvas text-slate-100">
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
      <aside className="sticky top-0 flex h-screen w-64 shrink-0 flex-col border-r border-white/5 bg-sidebar px-5 py-6">
        <p className="mb-3 px-3 text-[10px] font-semibold uppercase tracking-[0.18em] text-slate-500">
          ライブラリ
        </p>
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

        {tags.length > 0 && (
          <div className="mt-6 min-h-0 flex-1 overflow-y-auto pr-1">
            {tagCategories.map((category) => {
              const items = tags.filter((tag) => tag.category === category.value);
              if (items.length === 0) {
                return null;
              }
              return (
                <div className="mb-4" key={category.value}>
                  <p className="mb-2 px-3 text-[10px] font-semibold uppercase tracking-[0.18em] text-slate-500">
                    {category.label}
                  </p>
                  {items.map((tag) => (
                    <button
                      aria-pressed={tagIds.includes(tag.id)}
                      className={`flex w-full items-center justify-between gap-2 rounded-lg px-3 py-1.5 text-left text-xs transition ${
                        tagIds.includes(tag.id)
                          ? "bg-violet-400/15 text-violet-100"
                          : "text-slate-400 hover:bg-white/[0.04] hover:text-slate-200"
                      }`}
                      key={tag.id}
                      onClick={() =>
                        setTagIds((current) =>
                          current.includes(tag.id)
                            ? current.filter((id) => id !== tag.id)
                            : [...current, tag.id],
                        )
                      }
                      type="button"
                    >
                      <span className="truncate">{tag.name}</span>
                      <span className="text-[10px] text-slate-600">{tag.archiveCount}</span>
                    </button>
                  ))}
                </div>
              );
            })}
          </div>
        )}

        <div className="mt-auto shrink-0 rounded-xl border border-white/[0.06] bg-white/[0.025] p-4">
          <p className="text-xs font-medium text-slate-300">アプリ管理リポジトリ</p>
          {isLoading ? (
            <p className="mt-2 text-xs text-slate-500">状態を確認しています…</p>
          ) : repository ? (
            <>
              <p className="mt-2 break-all text-xs leading-5 text-emerald-300">
                接続済み
              </p>
              <p className="mt-1 break-all text-[11px] leading-4 text-slate-500">
                {repository.path}
              </p>
            </>
          ) : (
            <p className="mt-2 text-xs leading-5 text-slate-500">
              リポジトリを選択または新規作成してください。
            </p>
          )}
          <button
            className="mt-4 w-full rounded-lg border border-white/[0.08] px-3 py-2 text-xs font-medium text-slate-300 transition hover:bg-white/[0.05] disabled:cursor-wait disabled:opacity-50"
            disabled={isSelecting}
            onClick={selectRepository}
            type="button"
          >
            {isSelecting ? "処理中…" : repository ? "場所を変更" : "場所を選択"}
          </button>
        </div>
      </aside>

      <main className="min-w-0 flex-1">
        <header className="flex h-[76px] items-center justify-between border-b border-white/5 px-10">
          <div>
            <p className="text-xs text-slate-500">マイコレクション</p>
            <h1 className="mt-1 text-lg font-semibold tracking-tight">
              {navigation.find((item) => item.id === view)?.label}
            </h1>
          </div>
          <div className="flex items-center gap-3">
            <span className="rounded-full border border-violet-300/15 bg-violet-300/[0.07] px-3 py-1.5 text-[11px] font-medium text-violet-200">
              開発フェーズ 5
            </span>
            <div
              aria-label="ユーザー"
              className="grid size-9 place-items-center rounded-full bg-slate-700/70 text-xs font-semibold text-slate-200"
            >
              FL
            </div>
          </div>
        </header>

        <section className="mx-auto flex min-h-[calc(100vh-77px)] flex-col px-6 py-10">
          <div className="mb-8 flex flex-wrap items-end justify-between gap-4">
            <div>
              <h2 className="text-2xl font-semibold tracking-tight">コレクション</h2>
              <p className="mt-2 text-sm text-slate-500">
                アーカイブをまとめて、見つけやすく整理できます。
              </p>
            </div>
            {repository && (
              <label className="flex items-center gap-2 rounded-lg border border-white/[0.07] bg-white/[0.025] px-3 py-2 text-xs text-slate-300">
                <span aria-hidden="true" className="text-slate-500">
                  ⌕
                </span>
                <input
                  aria-label="ファイル名・タグで検索"
                  className="w-56 bg-transparent outline-none placeholder:text-slate-600"
                  onChange={(event) => setSearchText(event.target.value)}
                  placeholder="ファイル名・タグで検索"
                  type="search"
                  value={searchText}
                />
              </label>
            )}
          </div>

          {error && (
            <div
              className="mb-5 rounded-xl border border-rose-300/20 bg-rose-300/[0.06] px-4 py-3 text-sm text-rose-200"
              role="alert"
            >
              {error}
            </div>
          )}

          {importMessages.length > 0 && (
            <div
              aria-live="polite"
              className="mb-5 max-h-40 space-y-1 overflow-auto rounded-xl border border-white/[0.06] bg-white/[0.02] px-4 py-3 text-xs text-slate-300"
            >
              {importMessages.map((message, index) => (
                <p className="break-all" key={`${index}-${message}`}>
                  {message}
                </p>
              ))}
            </div>
          )}

          {repository ? (
            <LibraryView
              isFiltered={isFiltered}
              onError={setError}
              onMutated={bumpVersion}
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
        </section>
      </main>
      {repository && (
        <button
          aria-label={isImporting ? "インポート中" : "アーカイブをインポート"}
          className="fixed bottom-6 right-6 z-50 grid size-14 place-items-center rounded-full bg-violet-500 text-3xl leading-none text-white shadow-lg shadow-black/40 transition hover:bg-violet-400 disabled:cursor-wait disabled:opacity-60"
          disabled={isImporting}
          onClick={importArchives}
          title={isImporting ? "インポート中…" : "アーカイブをインポート（ウィンドウへのドロップも可）"}
          type="button"
        >
          <span aria-hidden="true" className={isImporting ? "animate-pulse text-xl" : ""}>
            {isImporting ? "…" : "+"}
          </span>
        </button>
      )}
    </div>
  );
}

export default App;

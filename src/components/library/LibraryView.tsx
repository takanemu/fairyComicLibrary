import { useCallback, useEffect, useState } from "react";
import {
  ArchiveQuery,
  ArchiveSummary,
  TagCategory,
  addTag,
  listArchives,
  removeTag,
  regenerateThumbnail,
  setFavorite,
} from "../../lib/library";
import { ArchiveCard } from "./ArchiveCard";
import { ArchiveDetail } from "./ArchiveDetail";

type Props = {
  query: ArchiveQuery;
  version: number;
  isFiltered: boolean;
  onMutated: () => void;
  onError: (message: string | null) => void;
  onCountChange: (count: number | null) => void;
};

export function LibraryView({
  query,
  version,
  isFiltered,
  onMutated,
  onError,
  onCountChange,
}: Props) {
  const [archives, setArchives] = useState<ArchiveSummary[] | null>(null);
  useEffect(() => {
    onCountChange(archives ? archives.length : null);
  }, [archives, onCountChange]);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    listArchives(query)
      .then((result) => {
        if (!cancelled) {
          setArchives(result);
        }
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          onError(`アーカイブ一覧を読み込めませんでした: ${String(reason)}`);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [query, version, onError]);

  const mutate = useCallback(
    async (action: () => Promise<unknown>, failure: string) => {
      try {
        await action();
        onError(null);
        onMutated();
      } catch (reason) {
        onError(`${failure}: ${String(reason)}`);
      }
    },
    [onError, onMutated],
  );

  const toggleFavorite = (archive: ArchiveSummary) =>
    mutate(
      () => setFavorite(archive.archiveId, !archive.isFavorite),
      "お気に入りを更新できませんでした",
    );

  const selected = archives?.find((archive) => archive.archiveId === selectedId);

  if (archives === null) {
    return <p className="py-16 text-center text-sm text-slate-500">読み込んでいます…</p>;
  }

  if (archives.length === 0) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center rounded-2xl border border-dashed border-white/[0.09] bg-white/[0.012] px-6 py-16 text-center">
        <div className="mb-6 grid size-[68px] place-items-center rounded-2xl border border-violet-300/10 bg-violet-300/[0.06] text-3xl text-violet-200/80">
          ◫
        </div>
        <h3 className="text-base font-medium text-slate-200">
          {isFiltered ? "条件に合うアーカイブがありません" : "コレクションはまだ空です"}
        </h3>
        <p className="mt-2 max-w-sm text-sm leading-6 text-slate-500">
          {isFiltered
            ? "検索語やフィルターを変更してください。"
            : "「アーカイブを選択」またはウィンドウへのドラッグ＆ドロップで作品を追加できます。"}
        </p>
      </div>
    );
  }

  return (
    <>
      <ul className="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-4">
        {archives.map((archive) => (
          <li key={archive.archiveId}>
            <ArchiveCard
              archive={archive}
              onSelect={() =>
                setSelectedId(archive.archiveId === selectedId ? null : archive.archiveId)
              }
              onToggleFavorite={() => void toggleFavorite(archive)}
              selected={archive.archiveId === selectedId}
            />
          </li>
        ))}
      </ul>
      {selected && (
        <ArchiveDetail
          archive={selected}
          onAddTag={(category: TagCategory, name: string) =>
            mutate(
              () => addTag(selected.archiveId, category, name),
              "タグを追加できませんでした",
            )
          }
          onClose={() => setSelectedId(null)}
          onRemoveTag={(tagId) =>
            mutate(
              () => removeTag(selected.archiveId, tagId),
              "タグを外せませんでした",
            )
          }
          onRegenerateCover={(entryName) =>
            mutate(
              () => regenerateThumbnail(selected.archiveId, entryName),
              "表紙を作り直せませんでした",
            )
          }
          onToggleFavorite={() => void toggleFavorite(selected)}
        />
      )}
    </>
  );
}

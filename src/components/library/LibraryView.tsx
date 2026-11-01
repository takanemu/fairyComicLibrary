import { useCallback, useEffect, useRef, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  ArchiveQuery,
  ArchiveSummary,
  TagCategory,
  addTag,
  listArchives,
  openInViewer,
  removeTag,
  regenerateThumbnail,
  isConvertible,
  setFavorite,
  unmanageArchives,
} from "../../lib/library";
import { ArchiveCard } from "./ArchiveCard";
import { ArchiveDetail } from "./ArchiveDetail";
import { ConvertDialog, ConvertResult } from "./ConvertDialog";
import { SelectionDetail } from "./SelectionDetail";

type Props = {
  query: ArchiveQuery;
  version: number;
  isFiltered: boolean;
  onMutated: () => void;
  onError: (message: string | null) => void;
  onCountChange: (count: number | null) => void;
  onConvertingChange: (busy: boolean) => void;
  onConvertComplete: (filename: string, result: ConvertResult) => void;
};

export function LibraryView({
  query,
  version,
  isFiltered,
  onMutated,
  onError,
  onCountChange,
  onConvertingChange,
  onConvertComplete,
}: Props) {
  const [archives, setArchives] = useState<ArchiveSummary[] | null>(null);
  useEffect(() => {
    onCountChange(archives ? archives.length : null);
  }, [archives, onCountChange]);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [convertingQueue, setConvertingQueue] = useState<ArchiveSummary[]>([]);
  const [bulkBusy, setBulkBusy] = useState(false);
  const selectionAnchor = useRef<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    listArchives(query)
      .then((result) => {
        if (!cancelled) {
          setArchives(result);
          const visibleIds = new Set(result.map((archive) => archive.archiveId));
          setSelectedIds((current) => current.filter((id) => visibleIds.has(id)));
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

  const mutateless = async (action: () => Promise<unknown>, failure: string) => {
    try {
      await action();
      onError(null);
      onMutated();
    } catch (reason) {
      onError(`${failure}: ${String(reason)}`);
    }
  };

  const toggleFavorite = (archive: ArchiveSummary) =>
    mutate(
      () => setFavorite(archive.archiveId, !archive.isFavorite),
      "お気に入りを更新できませんでした",
    );

  const selectArchive = (archiveId: string, additive: boolean, range: boolean) => {
    if (bulkBusy) return;
    if (range && archives) {
      const anchorIndex = archives.findIndex(
        (archive) => archive.archiveId === selectionAnchor.current,
      );
      const targetIndex = archives.findIndex((archive) => archive.archiveId === archiveId);
      if (anchorIndex >= 0 && targetIndex >= 0) {
        const [start, end] = [anchorIndex, targetIndex].sort((a, b) => a - b);
        const rangeIds = archives
          .slice(start, end + 1)
          .map((archive) => archive.archiveId);
        setSelectedIds((current) => [...new Set([...current, ...rangeIds])]);
        return;
      }
    }

    if (additive) {
      setSelectedIds((current) =>
        current.includes(archiveId)
          ? current.filter((id) => id !== archiveId)
          : [...current, archiveId],
      );
    } else {
      setSelectedIds([archiveId]);
    }
    selectionAnchor.current = archiveId;
  };

  const selected = archives?.filter((archive) => selectedIds.includes(archive.archiveId)) ?? [];

  const runBulk = async (
    targets: ArchiveSummary[],
    action: (archive: ArchiveSummary) => Promise<unknown>,
    failure: string,
  ): Promise<boolean> => {
    setBulkBusy(true);
    let succeeded = 0;
    let firstFailure: string | null = null;
    try {
      for (const archive of targets) {
        try {
          await action(archive);
          succeeded += 1;
        } catch (reason) {
          firstFailure ??= `${archive.filename}: ${String(reason)}`;
        }
      }
      if (succeeded > 0) onMutated();
      onError(
        firstFailure
          ? `${failure}（${succeeded}/${targets.length}件成功）: ${firstFailure}`
          : null,
      );
      return firstFailure === null;
    } finally {
      setBulkBusy(false);
    }
  };

  const addTagToSelection = async (category: TagCategory, name: string) => {
    return runBulk(
      selected,
      (archive) => addTag(archive.archiveId, category, name),
      "選択作品へのタグ追加に失敗しました",
    );
  };

  const removeTagFromSelection = (tagId: number) => {
    const targets = selected.filter((archive) =>
      archive.tags.some((tag) => tag.id === tagId),
    );
    void runBulk(
      targets,
      (archive) => removeTag(archive.archiveId, tagId),
      "選択作品からタグを外せませんでした",
    );
  };

  const toggleFavoritesForSelection = () => {
    const nextFavorite = selected.some((archive) => !archive.isFavorite);
    void runBulk(
      selected,
      (archive) => setFavorite(archive.archiveId, nextFavorite),
      "選択作品のお気に入りを更新できませんでした",
    );
  };

  const unmanageSelection = async () => {
    if (bulkBusy || selected.length === 0) return;
    setBulkBusy(true);
    try {
      const count = selected.length;
      const confirmed = await ask(
        `${count}件のアーカイブをダウンロードフォルダへ移動し、管理リポジトリから削除します。サムネイルとタグ付けも削除され、他の作品で使われないタグは一覧から削除されます。続行しますか？`,
        { title: "管理対象外にしますか？", kind: "warning" },
      );
      if (!confirmed) return;

      const summary = await unmanageArchives(
        selected.map((archive) => archive.archiveId),
      );
      if (summary.moved > 0) onMutated();
      const details = [
        summary.moved > 0 ? `${summary.moved}件を移動しました。` : null,
        summary.failed > 0
          ? `${summary.failed}件は移動できませんでした: ${summary.failures.join(" / ")}`
          : null,
        summary.warnings.length > 0
          ? `後処理に注意が必要です: ${summary.warnings.join(" / ")}`
          : null,
      ]
        .filter((message) => message !== null)
        .join(" ");
      onError(summary.failed > 0 || summary.warnings.length > 0 ? details : null);
    } catch (reason) {
      onError(`管理対象外への移動に失敗しました: ${String(reason)}`);
    } finally {
      setBulkBusy(false);
    }
  };

  const startConversion = () => {
    setConvertingQueue(selected.filter((archive) => isConvertible(archive.format)));
  };

  const closeConversion = () => {
    setConvertingQueue([]);
    onMutated();
  };

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
              onSelect={(additive, range) =>
                selectArchive(archive.archiveId, additive, range)
              }
              onOpen={() =>
                mutateless(
                  () => openInViewer(archive.archiveId),
                  "ビューアー起動または読書履歴の更新に失敗しました",
                )
              }
              onToggleFavorite={() => void toggleFavorite(archive)}
              selected={selectedIds.includes(archive.archiveId)}
            />
          </li>
        ))}
      </ul>
      {selected.length === 1 && (
        <ArchiveDetail
          archive={selected[0]}
          onAddTag={(category: TagCategory, name: string) =>
            mutate(
              () => addTag(selected[0].archiveId, category, name),
              "タグを追加できませんでした",
            )
          }
          onClose={() => setSelectedIds([])}
          onConvert={() => startConversion()}
          onUnmanage={() => void unmanageSelection()}
          unmanageBusy={bulkBusy}
          onRemoveTag={(tagId) =>
            mutate(
              () => removeTag(selected[0].archiveId, tagId),
              "タグを外せませんでした",
            )
          }
          onRegenerateCover={(entryName) =>
            mutate(
              () => regenerateThumbnail(selected[0].archiveId, entryName),
              "表紙を作り直せませんでした",
            )
          }
          onToggleFavorite={() => void toggleFavorite(selected[0])}
        />
      )}
      {selected.length > 1 && (
        <SelectionDetail
          archives={selected}
          busy={bulkBusy}
          onAddTag={addTagToSelection}
          onClose={() => setSelectedIds([])}
          onConvert={startConversion}
          onFavorite={toggleFavoritesForSelection}
          onRemoveTag={removeTagFromSelection}
          onUnmanage={() => void unmanageSelection()}
        />
      )}
      {convertingQueue.length > 0 && (
        <ConvertDialog
          archives={convertingQueue}
          onBusyChange={onConvertingChange}
          onClose={closeConversion}
          onComplete={(archive, result) =>
            onConvertComplete(archive.filename, result)
          }
        />
      )}
    </>
  );
}

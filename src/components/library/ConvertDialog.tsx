import { useEffect, useRef, useState } from "react";
import {
  ArchiveSummary,
  ConvertOutcome,
  ConvertPhase,
  ConvertProgress,
  cancelConversion,
  convertArchive,
  formatFileSize,
} from "../../lib/library";

export type ConvertResult = ConvertOutcome | { status: "failed"; message: string };

type Props = {
  archives: ArchiveSummary[];
  onBusyChange: (busy: boolean) => void;
  onComplete: (archive: ArchiveSummary, result: ConvertResult) => void;
  onClose: () => void;
};

const phaseLabels: Record<ConvertPhase, string> = {
  analyze: "アーカイブを解析しています",
  convert: "画像をAVIFへ変換しています",
  write: "7zを書き出しています",
  verify: "変換結果を検証しています",
  finalize: "保存先へ確定しています",
};

const phaseOrder: ConvertPhase[] = ["analyze", "convert", "write", "verify", "finalize"];

function formatDuration(seconds: number) {
  const total = Math.max(0, Math.round(seconds));
  const minutes = Math.floor(total / 60);
  return minutes > 0
    ? `${minutes}分${String(total % 60).padStart(2, "0")}秒`
    : `${total}秒`;
}

function resultLabel(result: ConvertResult) {
  switch (result.status) {
    case "converted":
      return `完了 ${formatFileSize(result.originalSize)} → ${formatFileSize(result.newSize)}`;
    case "duplicate":
      return "重複のため元を維持";
    case "notSmaller":
      return "サイズが小さくならないため元を維持";
    case "cancelled":
      return "中断";
    case "failed":
      return `失敗: ${result.message}`;
  }
}

export function ConvertDialog({ archives, onBusyChange, onComplete, onClose }: Props) {
  const [stage, setStage] = useState<"confirm" | "running" | "done">("confirm");
  const [speed, setSpeed] = useState(6);
  const [currentIndex, setCurrentIndex] = useState(0);
  const [progress, setProgress] = useState<ConvertProgress | null>(null);
  const [results, setResults] = useState<Array<{ archive: ArchiveSummary; result: ConvertResult }>>([]);
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState<string | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const convertStartedAt = useRef<number | null>(null);
  const startedAt = useRef(0);

  useEffect(() => {
    if (stage !== "running") {
      return;
    }
    const timer = window.setInterval(
      () => setElapsed((Date.now() - startedAt.current) / 1000),
      500,
    );
    return () => window.clearInterval(timer);
  }, [stage]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        if (stage === "confirm" || stage === "done") {
          onClose();
        }
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [stage, onClose]);

  async function start() {
    startedAt.current = Date.now();
    setElapsed(0);
    setStage("running");
    onBusyChange(true);

    const completed: Array<{ archive: ArchiveSummary; result: ConvertResult }> = [];
    for (const [index, archive] of archives.entries()) {
      setCurrentIndex(index);
      setProgress(null);
      convertStartedAt.current = null;
      let result: ConvertResult;
      try {
        result = await convertArchive(archive.archiveId, speed, (update) => {
          if (update.phase === "convert" && convertStartedAt.current === null) {
            convertStartedAt.current = Date.now();
          }
          setProgress(update);
        });
      } catch (reason) {
        result = { status: "failed", message: String(reason) };
      }
      completed.push({ archive, result });
      setResults([...completed]);
      onComplete(archive, result);
      if (result.status === "cancelled") {
        break;
      }
    }

    setStage("done");
    onBusyChange(false);
  }

  async function cancel() {
    setCancelling(true);
    setCancelError(null);
    try {
      await cancelConversion();
    } catch (reason) {
      setCancelling(false);
      setCancelError(`中断を要求できませんでした: ${String(reason)}`);
    }
  }

  const currentArchive = archives[currentIndex];
  const ratio =
    progress && progress.totalBytes > 0 ? progress.doneBytes / progress.totalBytes : 0;
  const convertElapsed =
    convertStartedAt.current === null ? 0 : (Date.now() - convertStartedAt.current) / 1000;
  const remaining =
    progress?.phase === "convert" && ratio >= 0.03 && convertElapsed >= 3
      ? (convertElapsed * (1 - ratio)) / ratio
      : null;
  const convertedCount = results.filter(({ result }) => result.status === "converted").length;
  const failedCount = results.filter(
    ({ result }) => result.status === "failed" || result.status === "notSmaller",
  ).length;
  const skippedCount = archives.length - results.length;

  return (
    <div
      aria-label="7z + AVIFへ変換"
      aria-modal="true"
      className="fixed inset-0 z-[70] grid place-items-center bg-black/80 p-6"
      role="dialog"
    >
      <div className="w-full max-w-md rounded-2xl border border-white/[0.08] bg-[#15171d] p-6 shadow-2xl">
        <h2 className="text-sm font-semibold text-slate-100">7z + AVIF へ変換</h2>

        {stage === "confirm" && (
          <>
            <p className="mt-1 text-xs text-slate-500">{archives.length} 件を順番に変換します。</p>
            <ul className="mt-3 max-h-28 space-y-1 overflow-y-auto rounded-lg border border-white/[0.06] bg-white/[0.02] p-2">
              {archives.map((archive) => (
                <li className="truncate text-xs text-slate-300" key={archive.archiveId} title={archive.filename}>
                  {archive.filename}
                </li>
              ))}
            </ul>
            <label className="mt-4 flex items-center justify-between gap-3 text-xs text-slate-300">
              <span>AVIFエンコード速度</span>
              <div className="relative">
                <select
                  aria-label="AVIFエンコード速度"
                  className="w-full appearance-none rounded-lg border border-white/[0.07] bg-slate-800 py-2 pl-3 pr-7 text-xs text-slate-100 outline-none transition focus:border-violet-300/50"
                  onChange={(event) => setSpeed(Number(event.target.value))}
                  value={speed}
                >
                  {Array.from({ length: 10 }, (_, index) => index + 1).map((value) => (
                    <option key={value} value={value}>
                      {value}{value === 6 ? "（標準）" : ""}
                    </option>
                  ))}
                </select>
                <span
                  aria-hidden="true"
                  className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 text-[10px] text-slate-300"
                >
                  ▾
                </span>
              </div>
            </label>
            <p className="mt-1 text-[11px] leading-5 text-slate-500">
              1は低速・高圧縮、10は高速・ファイルサイズ大きめです。品質は70固定です。
            </p>
            <ul className="mt-3 list-disc space-y-1.5 pl-5 text-xs leading-5 text-slate-300">
              <li>PNG/JPEGをAVIFへ変換し、7zで再アーカイブします。</li>
              <li>画質は非可逆です。JPEGは再圧縮により二重に劣化します。</li>
              <li>変換成功時は元アーカイブを削除し、お気に入りとタグを引き継ぎます。</li>
              <li>変換中は他の操作ができません。中断すると残りの変換も行いません。</li>
            </ul>
            <div className="mt-6 flex justify-end gap-2">
              <button
                className="rounded-lg border border-white/[0.08] px-4 py-2 text-xs text-slate-300 hover:bg-white/[0.05]"
                onClick={onClose}
                type="button"
              >
                キャンセル
              </button>
              <button
                className="rounded-lg bg-violet-500 px-4 py-2 text-xs font-medium text-white hover:bg-violet-400"
                onClick={() => void start()}
                type="button"
              >
                {archives.length} 件の変換を開始
              </button>
            </div>
          </>
        )}

        {stage === "running" && currentArchive && (
          <>
            <p className="mt-1 truncate text-xs text-slate-500" title={currentArchive.filename}>
              {currentIndex + 1} / {archives.length} 件: {currentArchive.filename}
            </p>
            <ol className="mt-4 flex gap-1" aria-label="工程">
              {phaseOrder.map((phase) => {
                const current = progress ? phaseOrder.indexOf(progress.phase) : 0;
                const index = phaseOrder.indexOf(phase);
                return (
                  <li
                    className={`h-1 flex-1 rounded-full ${
                      index < current
                        ? "bg-violet-400"
                        : index === current
                          ? "bg-violet-400/60"
                          : "bg-white/10"
                    }`}
                    key={phase}
                  />
                );
              })}
            </ol>
            <p className="mt-3 text-xs text-slate-200">
              {cancelling ? "中断しています…" : phaseLabels[progress?.phase ?? "analyze"]}
            </p>
            <div className="mt-3 h-2 overflow-hidden rounded-full bg-white/10">
              <div
                className="h-full rounded-full bg-violet-400 transition-[width] duration-100"
                style={{ width: `${Math.round(ratio * 100)}%` }}
              />
            </div>
            <div className="mt-2 flex justify-between text-[11px] text-slate-400">
              <span>
                {progress && progress.totalEntries > 0
                  ? `${progress.doneEntries} / ${progress.totalEntries} ファイル（${Math.round(ratio * 100)}%）`
                  : ""}
              </span>
              <span>
                経過 {formatDuration(elapsed)}
                {remaining !== null ? ` ・ 残り約 ${formatDuration(remaining)}` : ""}
              </span>
            </div>
            <p className="mt-2 h-4 truncate text-[11px] text-slate-500">
              {progress?.currentName ?? ""}
            </p>
            {cancelError && <p className="mt-2 text-xs text-rose-300">{cancelError}</p>}
            <div className="mt-5 flex justify-end">
              <button
                className="rounded-lg border border-white/[0.08] px-4 py-2 text-xs text-slate-300 hover:bg-white/[0.05] disabled:opacity-40"
                disabled={cancelling || !(progress?.cancellable ?? true)}
                onClick={() => void cancel()}
                type="button"
              >
                中断
              </button>
            </div>
          </>
        )}

        {stage === "done" && (
          <>
            <p className="mt-2 text-xs text-slate-300">
              {results.length} / {archives.length} 件を処理しました。変換成功 {convertedCount} 件
              {failedCount > 0 ? `、失敗・サイズ不変 ${failedCount} 件` : ""}
              {skippedCount > 0 ? `、未処理 ${skippedCount} 件` : ""}。
            </p>
            <ul className="mt-3 max-h-64 space-y-2 overflow-y-auto rounded-lg border border-white/[0.06] bg-white/[0.02] p-3 text-xs">
              {results.map(({ archive, result }) => (
                <li key={archive.archiveId}>
                  <p className="truncate text-slate-300" title={archive.filename}>
                    {archive.filename}
                  </p>
                  <p
                    className={`mt-0.5 break-words ${
                      result.status === "converted" ? "text-emerald-300" : "text-amber-200"
                    }`}
                  >
                    {resultLabel(result)}
                  </p>
                  {result.status === "converted" && result.warning && (
                    <p className="mt-0.5 text-amber-200">{result.warning}</p>
                  )}
                </li>
              ))}
            </ul>
            {skippedCount > 0 && (
              <p className="mt-2 text-[11px] text-slate-500">
                中断後の作品は元のアーカイブのままです。
              </p>
            )}
            <div className="mt-6 flex justify-end">
              <button
                className="rounded-lg bg-violet-500 px-4 py-2 text-xs font-medium text-white hover:bg-violet-400"
                onClick={onClose}
                type="button"
              >
                閉じる
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

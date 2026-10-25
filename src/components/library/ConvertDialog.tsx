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
  archive: ArchiveSummary;
  onBusyChange: (busy: boolean) => void;
  onComplete: (result: ConvertResult) => void;
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

export function ConvertDialog({ archive, onBusyChange, onComplete, onClose }: Props) {
  const [stage, setStage] = useState<"confirm" | "running" | "done">("confirm");
  const [progress, setProgress] = useState<ConvertProgress | null>(null);
  const [result, setResult] = useState<ConvertResult | null>(null);
  const [cancelling, setCancelling] = useState(false);
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
        if (stage === "confirm") {
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
    let finished: ConvertResult;
    try {
      finished = await convertArchive(archive.archiveId, (update) => {
        if (update.phase === "convert" && convertStartedAt.current === null) {
          convertStartedAt.current = Date.now();
        }
        setProgress(update);
      });
    } catch (reason) {
      finished = { status: "failed", message: String(reason) };
    }
    setResult(finished);
    setStage("done");
    onBusyChange(false);
    onComplete(finished);
  }

  async function cancel() {
    setCancelling(true);
    await cancelConversion().catch(() => undefined);
  }

  const ratio =
    progress && progress.totalBytes > 0 ? progress.doneBytes / progress.totalBytes : 0;
  const convertElapsed =
    convertStartedAt.current === null ? 0 : (Date.now() - convertStartedAt.current) / 1000;
  const remaining =
    progress?.phase === "convert" && ratio >= 0.03 && convertElapsed >= 3
      ? (convertElapsed * (1 - ratio)) / ratio
      : null;

  return (
    <div
      aria-label="7z + AVIFへ変換"
      aria-modal="true"
      className="fixed inset-0 z-[70] grid place-items-center bg-black/80 p-6"
      role="dialog"
    >
      <div className="w-full max-w-md rounded-2xl border border-white/[0.08] bg-[#15171d] p-6 shadow-2xl">
        <h2 className="text-sm font-semibold text-slate-100">7z + AVIF へ変換</h2>
        <p className="mt-1 truncate text-xs text-slate-500" title={archive.filename}>
          {archive.filename}
        </p>

        {stage === "confirm" && (
          <>
            <ul className="mt-4 list-disc space-y-1.5 pl-5 text-xs leading-5 text-slate-300">
              <li>PNG/JPEGをAVIF（品質70）へ変換し、7zで再アーカイブします。</li>
              <li>画質は非可逆です。JPEGは再圧縮により二重に劣化します。</li>
              <li>
                内容が変わるため別のアーカイブとして登録し直します。お気に入りとタグは引き継ぎます。
              </li>
              <li className="text-amber-200">
                変換に成功すると、元のアーカイブ（{formatFileSize(archive.fileSize)}）は削除されます。
              </li>
              <li>枚数が多いと時間がかかります。変換中は他の操作ができません。</li>
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
                変換を開始
              </button>
            </div>
          </>
        )}

        {stage === "running" && (
          <>
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

        {stage === "done" && result && (
          <>
            <div className="mt-4 space-y-1.5 text-xs leading-5 text-slate-300">
              {result.status === "converted" && (
                <>
                  <p className="font-medium text-emerald-300">変換が完了しました。</p>
                  <p>
                    {formatFileSize(result.originalSize)} → {formatFileSize(result.newSize)}（
                    {Math.round((1 - result.newSize / result.originalSize) * 100)}% 削減）
                  </p>
                  <p>
                    AVIFへ変換: {result.convertedImages} 枚 / そのまま: {result.unchangedEntries} 件
                  </p>
                  {result.warning && <p className="text-amber-200">{result.warning}</p>}
                </>
              )}
              {result.status === "duplicate" && (
                <p>同じ内容のアーカイブが既に登録されているため、元のアーカイブを残しました。</p>
              )}
              {result.status === "notSmaller" && (
                <p>
                  変換後（{formatFileSize(result.newSize)}）が元（
                  {formatFileSize(result.originalSize)}）より小さくならなかったため、元のアーカイブを残しました。
                </p>
              )}
              {result.status === "cancelled" && (
                <p>変換を中断しました。元のアーカイブはそのまま残っています。</p>
              )}
              {result.status === "failed" && (
                <p className="break-all text-rose-300">
                  変換できませんでした（元のアーカイブは残っています）: {result.message}
                </p>
              )}
            </div>
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

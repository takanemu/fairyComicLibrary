import { useEffect, useRef, useState } from "react";
import { listCoverCandidates } from "../../lib/library";

type Props = {
  archiveId: string;
  onSelect: (entryName: string) => void;
  onClose: () => void;
};

export function CoverPicker({ archiveId, onSelect, onClose }: Props) {
  const [names, setNames] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    let cancelled = false;
    listCoverCandidates(archiveId)
      .then((result) => {
        if (!cancelled) {
          setNames(result);
        }
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          setError(String(reason));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [archiveId]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        closeRef.current();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, []);

  return (
    <div
      aria-label="表紙にする画像を選択"
      aria-modal="true"
      className="fixed inset-0 z-[60] grid place-items-center bg-black/70 p-6"
      role="dialog"
    >
      <div className="flex max-h-[85vh] w-full max-w-3xl flex-col rounded-2xl border border-white/[0.08] bg-[#15171d] p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-sm font-semibold text-slate-100">
            表紙にする画像を選択
          </h2>
          <button
            aria-label="閉じる"
            className="grid size-8 place-items-center rounded-full text-slate-300 hover:bg-white/[0.08]"
            onClick={onClose}
            type="button"
          >
            ✕
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto">
          {error ? (
            <p className="text-sm text-rose-300">
              画像の一覧を取得できませんでした: {error}
            </p>
          ) : names === null ? (
            <p className="text-sm text-slate-400">アーカイブを読み込んでいます…</p>
          ) : names.length === 0 ? (
            <p className="text-sm text-slate-400">
              表紙に使える画像が見つかりませんでした。
            </p>
          ) : (
            <ul className="divide-y divide-white/[0.06]">
              {names.map((name) => (
                <li key={name}>
                  <button
                    className="w-full truncate px-3 py-2 text-left text-xs text-slate-200 hover:bg-white/[0.06]"
                    onClick={() => onSelect(name)}
                    title={name}
                    type="button"
                  >
                    {name}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}

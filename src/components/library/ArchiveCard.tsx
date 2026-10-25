import { useState } from "react";
import { ArchiveSummary, thumbnailUrl } from "../../lib/library";

type Props = {
  archive: ArchiveSummary;
  selected: boolean;
  onSelect: () => void;
  onToggleFavorite: () => void;
};

export function ArchiveCard({ archive, selected, onSelect, onToggleFavorite }: Props) {
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  const url = thumbnailUrl(archive.archiveId);

  return (
    <div
      className={`group relative overflow-hidden rounded-xl border bg-white/[0.025] transition ${
        selected
          ? "border-violet-300/50 ring-1 ring-violet-300/30"
          : "border-white/[0.06] hover:border-white/[0.14]"
      }`}
    >
      <button
        aria-label={`${archive.filename} の詳細を開く`}
        className="block w-full text-left"
        onClick={onSelect}
        type="button"
      >
        <div className="grid aspect-[2/3] w-full place-items-center overflow-hidden bg-black/20">
          {archive.hasThumbnail && failedUrl !== url ? (
            <img
              alt=""
              className="size-full object-cover"
              loading="lazy"
              onError={() => setFailedUrl(url)}
              src={url}
            />
          ) : (
            <span aria-hidden="true" className="text-3xl text-slate-600">
              ◫
            </span>
          )}
        </div>
        <div className="px-3 py-2.5">
          <p className="line-clamp-2 break-all text-xs font-medium leading-5 text-slate-200">
            {archive.filename}
          </p>
          <p className="mt-1 text-[10px] uppercase tracking-wide text-slate-500">
            {archive.pageCount === null ? "ページ数不明" : `${archive.pageCount} ページ`}
          </p>
        </div>
      </button>
      <button
        aria-label={archive.isFavorite ? "お気に入りから外す" : "お気に入りに追加"}
        aria-pressed={archive.isFavorite}
        className={`absolute right-2 top-2 grid size-8 place-items-center rounded-full bg-black/55 text-base backdrop-blur transition hover:bg-black/75 ${
          archive.isFavorite
            ? "text-rose-300"
            : "text-slate-300 opacity-0 focus:opacity-100 group-hover:opacity-100"
        }`}
        onClick={onToggleFavorite}
        type="button"
      >
        {archive.isFavorite ? "♥" : "♡"}
      </button>
    </div>
  );
}

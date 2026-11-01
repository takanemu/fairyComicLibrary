import { useEffect, useRef, useState } from "react";
import { ArchiveSummary, thumbnailUrl } from "../../lib/library";

type Props = {
  archive: ArchiveSummary;
  selected: boolean;
  onSelect: () => void;
  onOpen: () => void;
  onToggleFavorite: () => void;
};

function filenameWithoutTags(filename: string) {
  const trimmedFilename = filename.trimStart();
  if (!trimmedFilename.startsWith("(")) return filename;

  const classificationEnd = trimmedFilename.indexOf(")");
  if (classificationEnd < 0) return filename;
  const classification = trimmedFilename.slice(1, classificationEnd).trim();
  const afterClassification = trimmedFilename.slice(classificationEnd + 1).trimStart();
  if (!afterClassification.startsWith("[")) return filename;

  const authorEnd = afterClassification.indexOf("]");
  if (authorEnd < 0) return filename;
  const author = afterClassification.slice(1, authorEnd).trim();
  const isValidTag = (tag: string) => tag.length > 0 && Array.from(tag).length <= 100;
  if (!isValidTag(classification) || !isValidTag(author)) return filename;

  return afterClassification.slice(authorEnd + 1).trimStart();
}

export function ArchiveCard({ archive, selected, onSelect, onOpen, onToggleFavorite }: Props) {
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  const url = thumbnailUrl(archive.archiveId);
  const displayFilename = filenameWithoutTags(archive.filename);
  const clickTimer = useRef<number | null>(null);

  const cancelPendingSelect = () => {
    if (clickTimer.current !== null) {
      window.clearTimeout(clickTimer.current);
      clickTimer.current = null;
    }
  };

  useEffect(() => cancelPendingSelect, []);

  // ダブルクリックでは詳細ペインを開閉しないよう、選択を遅延して取り消せるようにする
  const handleClick = () => {
    cancelPendingSelect();
    clickTimer.current = window.setTimeout(() => {
      clickTimer.current = null;
      onSelect();
    }, 250);
  };

  const handleDoubleClick = () => {
    cancelPendingSelect();
    onOpen();
  };

  return (
    <div
      className={`group relative overflow-hidden rounded-xl border bg-white/[0.025] transition ${
        selected
          ? "border-violet-300/50 ring-1 ring-violet-300/30"
          : "border-white/[0.06] hover:border-white/[0.14]"
      }`}
    >
      <button
        aria-label={`${archive.filename} の詳細を表示`}
        className="block w-full text-left"
        onClick={handleClick}
        onDoubleClick={handleDoubleClick}
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
            {displayFilename}
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

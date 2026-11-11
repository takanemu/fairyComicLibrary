import { useEffect, useRef, useState } from "react";
import { ArchiveSummary, isEbook, isVideo, thumbnailUrl } from "../../lib/library";
import thumbnailUnavailable from "../../assets/thumbnail-unavailable.svg";

type Props = {
  archive: ArchiveSummary;
  selected: boolean;
  onSelect: (additive: boolean, range: boolean) => void;
  onOpen: () => void;
  onToggleFavorite: () => void;
};

function filenameWithoutTags(filename: string) {
  const isValidTag = (tag: string) => tag.length > 0 && Array.from(tag).length <= 100;
  let remaining = filename.trimStart();
  const preservedTags: string[] = [];
  let removedTag = false;

  while (remaining.startsWith("(") || remaining.startsWith("[") || remaining.startsWith("{")) {
    const opening = remaining[0];
    const closing = opening === "(" ? ")" : opening === "[" ? "]" : "}";
    const tagEnd = remaining.indexOf(closing);
    if (tagEnd < 0) break;

    const tag = remaining.slice(1, tagEnd).trim();
    if (isValidTag(tag)) {
      removedTag = true;
    } else {
      preservedTags.push(remaining.slice(0, tagEnd + 1));
    }
    remaining = remaining.slice(tagEnd + 1).trimStart();
  }

  if (!removedTag) return filename;
  return [...preservedTags, remaining].filter(Boolean).join(" ");
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
  const handleClick = (event: React.MouseEvent<HTMLButtonElement>) => {
    const additive = event.ctrlKey || event.metaKey;
    const range = event.shiftKey;
    cancelPendingSelect();
    clickTimer.current = window.setTimeout(() => {
      clickTimer.current = null;
      onSelect(additive, range);
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
        aria-pressed={selected}
        className="block w-full text-left"
        onClick={handleClick}
        onDoubleClick={handleDoubleClick}
        title="クリックで選択、Ctrl/⌘+クリックで複数選択、Shift+クリックで範囲選択"
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
            <img
              alt="サムネイル未指定"
              className="size-full object-cover"
              src={thumbnailUnavailable}
            />
          )}
        </div>
        <div className="px-3 py-2.5">
          <p className="line-clamp-2 break-all text-xs font-medium leading-5 text-slate-200">
            {displayFilename}
          </p>
          <p className="mt-1 text-[10px] uppercase tracking-wide text-slate-500">
            {isVideo(archive.format)
              ? "動画"
              : isEbook(archive.format)
              ? "電子書籍"
              : archive.pageCount === null
                ? "ページ数不明"
                : `${archive.pageCount} ページ`}
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

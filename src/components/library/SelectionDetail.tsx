import { FormEvent, useEffect, useState } from "react";
import {
  ArchiveSummary,
  TagCategory,
  isConvertible,
  tagCategories,
} from "../../lib/library";

type Props = {
  archives: ArchiveSummary[];
  busy: boolean;
  onClose: () => void;
  onFavorite: () => void;
  onAddTag: (category: TagCategory, name: string) => Promise<boolean>;
  onRemoveTag: (tagId: number) => void;
  onConvert: () => void;
  onUnmanage: () => void;
};

export function SelectionDetail({
  archives,
  busy,
  onClose,
  onFavorite,
  onAddTag,
  onRemoveTag,
  onConvert,
  onUnmanage,
}: Props) {
  const [category, setCategory] = useState<TagCategory>("author");
  const [name, setName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const allFavorite = archives.every((archive) => archive.isFavorite);
  const convertibleCount = archives.filter((archive) => isConvertible(archive.format)).length;
  const tags = new Map<
    number,
    { name: string; category: TagCategory; archiveCount: number }
  >();

  for (const archive of archives) {
    for (const tag of archive.tags) {
      const current = tags.get(tag.id);
      if (current) {
        current.archiveCount += 1;
      } else {
        tags.set(tag.id, {
          name: tag.name,
          category: tag.category,
          archiveCount: 1,
        });
      }
    }
  }

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!name.trim() || submitting || busy) return;

    setSubmitting(true);
    try {
      if (await onAddTag(category, name)) {
        setName("");
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <aside
      aria-label="選択した作品の詳細"
      className="fixed bottom-0 right-0 top-[49px] z-40 flex w-80 flex-col overflow-y-auto border-l border-white/[0.07] bg-black/90 p-5 pb-28 shadow-2xl backdrop-blur-md"
    >
      <div className="mb-4 flex items-center justify-between">
        <div>
          <h2 className="text-sm font-semibold text-slate-100">{archives.length} 件を選択中</h2>
          <p className="mt-1 text-[11px] text-slate-500">
            Ctrl/⌘+クリックで選択を追加・解除
          </p>
        </div>
        <button
          aria-label="選択を閉じる"
          className="grid size-8 place-items-center rounded-full text-sm text-slate-300 transition hover:bg-white/[0.08]"
          onClick={onClose}
          type="button"
        >
          ✕
        </button>
      </div>

      <ul className="max-h-36 space-y-1 overflow-y-auto rounded-lg border border-white/[0.06] bg-white/[0.02] p-2">
        {archives.map((archive) => (
          <li
            className="truncate text-xs text-slate-300"
            key={archive.archiveId}
            title={archive.filename}
          >
            {archive.filename}
          </li>
        ))}
      </ul>

      <button
        className={`mt-4 rounded-lg border px-3 py-2 text-xs font-medium transition disabled:opacity-50 ${
          allFavorite
            ? "border-rose-300/30 bg-rose-300/10 text-rose-200"
            : "border-white/[0.08] text-slate-300 hover:bg-white/[0.05]"
        }`}
        disabled={busy}
        onClick={onFavorite}
        type="button"
      >
        {allFavorite ? "♥ 選択作品をお気に入りから外す" : "♡ 選択作品をお気に入りに追加"}
      </button>

      <button
        className="mt-2 rounded-lg border border-white/[0.08] px-3 py-2 text-xs font-medium text-white transition hover:bg-white/[0.05] disabled:opacity-50"
        disabled={busy || convertibleCount === 0}
        onClick={onConvert}
        title="変換可能なZIP/CBZを順番に変換します"
        type="button"
      >
        7z + AVIF へ変換 ({convertibleCount} 件)
      </button>
      {convertibleCount > 0 && convertibleCount < archives.length && (
        <p className="mt-1 text-[10px] text-slate-500">
          ZIP/CBZ形式のみを順番に変換します。
        </p>
      )}

      <button
        className="mt-2 rounded-lg border border-amber-300/20 px-3 py-2 text-xs font-medium text-amber-100 transition hover:bg-amber-300/[0.08] disabled:opacity-50"
        disabled={busy}
        onClick={onUnmanage}
        type="button"
      >
        管理対象外にする（ダウンロードへ移動）{` (${archives.length} 件)`}
      </button>

      <h3 className="mb-2 mt-6 text-xs font-semibold text-slate-400">選択作品のタグ</h3>
      {tags.size === 0 ? (
        <p className="text-xs text-slate-600">選択作品にタグはありません。</p>
      ) : (
        <ul className="flex flex-col gap-1.5">
          {[...tags.entries()].map(([tagId, tag]) => (
            <li
              className="flex min-w-0 items-center gap-1 rounded-full border border-white/[0.08] bg-white/[0.04] py-1 pl-2.5 pr-1 text-[11px] text-slate-200"
              key={tagId}
            >
              <span className="shrink-0 text-slate-500">
                {tagCategories.find((item) => item.value === tag.category)?.label}
              </span>
              <span className="min-w-0 flex-1 truncate" title={tag.name}>
                {tag.name}
              </span>
              <span className="shrink-0 text-slate-500">{tag.archiveCount}/{archives.length}</span>
              <button
                aria-label={`タグ ${tag.name} を選択作品から外す`}
                className="grid size-5 shrink-0 place-items-center rounded-full text-slate-400 hover:bg-white/10 disabled:opacity-50"
                disabled={busy}
                onClick={() => onRemoveTag(tagId)}
                type="button"
              >
                ✕
              </button>
            </li>
          ))}
        </ul>
      )}

      <form className="mt-4 flex flex-col gap-2" onSubmit={(event) => void submit(event)}>
        <div className="flex gap-2">
          <div className="relative">
            <select
              aria-label="タグの分類"
              className="w-full appearance-none rounded-lg border border-white/[0.07] bg-slate-800 py-2 pl-3 pr-7 text-xs text-slate-100 outline-none transition focus:border-violet-300/50"
              disabled={busy || submitting}
              onChange={(event) => setCategory(event.target.value as TagCategory)}
              value={category}
            >
              {tagCategories.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
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
          <label className="flex min-w-0 flex-1 items-center rounded-lg border border-white/[0.07] bg-white/[0.025] px-3 py-2 text-xs text-slate-300">
            <input
              aria-label="タグ名"
              className="w-full min-w-0 bg-transparent outline-none placeholder:text-slate-600"
              maxLength={100}
              onChange={(event) => setName(event.target.value)}
              placeholder="タグ名を入力"
              value={name}
            />
          </label>
        </div>
        <button
          className="rounded-lg bg-violet-400/15 px-3 py-2 text-xs font-medium text-violet-200 transition hover:bg-violet-400/20 disabled:opacity-50"
          disabled={busy || submitting || !name.trim()}
          type="submit"
        >
          {submitting ? "追加中…" : "選択作品にタグを追加"}
        </button>
      </form>
    </aside>
  );
}

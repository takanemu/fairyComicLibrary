import { FormEvent, useState } from "react";
import {
  ArchiveSummary,
  TagCategory,
  formatFileSize,
  tagCategories,
  thumbnailUrl,
} from "../../lib/library";

type Props = {
  archive: ArchiveSummary;
  onClose: () => void;
  onToggleFavorite: () => void;
  onAddTag: (category: TagCategory, name: string) => Promise<void>;
  onRemoveTag: (tagId: number) => Promise<void>;
};

export function ArchiveDetail({
  archive,
  onClose,
  onToggleFavorite,
  onAddTag,
  onRemoveTag,
}: Props) {
  const [category, setCategory] = useState<TagCategory>("author");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!name.trim()) {
      return;
    }
    setBusy(true);
    try {
      await onAddTag(category, name);
      setName("");
    } finally {
      setBusy(false);
    }
  }

  return (
    <aside
      aria-label="アーカイブの詳細"
      className="fixed inset-y-0 right-0 z-40 flex w-80 flex-col overflow-y-auto border-l border-white/[0.07] bg-sidebar p-5 pb-28 shadow-2xl"
    >
      <div className="mb-4 flex items-center justify-between">
        <h2 className="text-sm font-semibold text-slate-200">詳細</h2>
        <button
          aria-label="閉じる"
          className="grid size-8 place-items-center rounded-lg text-slate-400 hover:bg-white/[0.06]"
          onClick={onClose}
          type="button"
        >
          ✕
        </button>
      </div>

      {archive.hasThumbnail && (
        <img
          alt=""
          className="mb-4 w-full rounded-xl border border-white/[0.06] object-cover"
          src={thumbnailUrl(archive.archiveId)}
        />
      )}
      <p className="break-all text-sm font-medium leading-6 text-slate-100">
        {archive.filename}
      </p>
      <dl className="mt-3 space-y-1 text-xs text-slate-500">
        <div className="flex justify-between">
          <dt>形式</dt>
          <dd className="uppercase text-slate-300">{archive.format}</dd>
        </div>
        <div className="flex justify-between">
          <dt>サイズ</dt>
          <dd className="text-slate-300">{formatFileSize(archive.fileSize)}</dd>
        </div>
        <div className="flex justify-between">
          <dt>追加日</dt>
          <dd className="text-slate-300">
            {new Date(archive.importedAt).toLocaleString("ja-JP")}
          </dd>
        </div>
      </dl>

      <button
        aria-pressed={archive.isFavorite}
        className={`mt-5 rounded-lg border px-3 py-2 text-xs font-medium transition ${
          archive.isFavorite
            ? "border-rose-300/30 bg-rose-300/10 text-rose-200"
            : "border-white/[0.08] text-slate-300 hover:bg-white/[0.05]"
        }`}
        onClick={onToggleFavorite}
        type="button"
      >
        {archive.isFavorite ? "♥ お気に入り済み" : "♡ お気に入りに追加"}
      </button>

      <h3 className="mb-2 mt-6 text-xs font-semibold text-slate-400">タグ</h3>
      {archive.tags.length === 0 ? (
        <p className="text-xs text-slate-600">タグはまだありません。</p>
      ) : (
        <ul className="flex flex-wrap gap-1.5">
          {archive.tags.map((tag) => (
            <li
              className="flex items-center gap-1 rounded-full border border-white/[0.08] bg-white/[0.04] py-1 pl-2.5 pr-1 text-[11px] text-slate-200"
              key={tag.id}
            >
              <span className="text-slate-500">
                {tagCategories.find((item) => item.value === tag.category)?.label}
              </span>
              {tag.name}
              <button
                aria-label={`タグ ${tag.name} を外す`}
                className="grid size-5 place-items-center rounded-full text-slate-400 hover:bg-white/10"
                onClick={() => void onRemoveTag(tag.id)}
                type="button"
              >
                ✕
              </button>
            </li>
          ))}
        </ul>
      )}

      <form className="mt-4 flex flex-col gap-2" onSubmit={submit}>
        <div className="flex gap-2">
          <select
            aria-label="タグの分類"
            className="rounded-lg border border-white/[0.08] bg-canvas px-2 py-2 text-xs text-slate-200"
            onChange={(event) => setCategory(event.target.value as TagCategory)}
            value={category}
          >
            {tagCategories.map((item) => (
              <option key={item.value} value={item.value}>
                {item.label}
              </option>
            ))}
          </select>
          <input
            aria-label="タグ名"
            className="min-w-0 flex-1 rounded-lg border border-white/[0.08] bg-canvas px-3 py-2 text-xs text-slate-200 placeholder:text-slate-600"
            maxLength={100}
            onChange={(event) => setName(event.target.value)}
            placeholder="タグ名を入力"
            value={name}
          />
        </div>
        <button
          className="rounded-lg bg-violet-400/15 px-3 py-2 text-xs font-medium text-violet-200 transition hover:bg-violet-400/20 disabled:opacity-50"
          disabled={busy || !name.trim()}
          type="submit"
        >
          タグを追加
        </button>
      </form>
    </aside>
  );
}

import { CoverPicker } from "./CoverPicker";
import { FormEvent, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArchiveSummary,
  TagCategory,
  formatFileSize,
  formatDuration,
  getVideoDuration,
  isEbook,
  isVideo,
  isConvertible,
  tagCategories,
  thumbnailUrl,
} from "../../lib/library";
import thumbnailUnavailable from "../../assets/thumbnail-unavailable.svg";

type Props = {
  archive: ArchiveSummary;
  onClose: () => void;
  activeTagIds: number[];
  onToggleTagFilter: (tagId: number) => void;
  onOpen: () => void;
  onToggleFavorite: () => void;
  onAddTag: (category: TagCategory, name: string) => Promise<void>;
  onRemoveTag: (tagId: number) => Promise<void>;
  onRegenerateCover: (entryName: string | null) => Promise<void>;
  onSetPackageImage: (path: string) => Promise<void>;
  onConvert: () => void;
  onUnmanage: () => void;
  unmanageBusy: boolean;
};

export function ArchiveDetail({
  archive,
  onClose,
  activeTagIds,
  onToggleTagFilter,
  onOpen,
  onToggleFavorite,
  onAddTag,
  onRemoveTag,
  onRegenerateCover,
  onSetPackageImage,
  onConvert,
  onUnmanage,
  unmanageBusy,
}: Props) {
  const [coverBusy, setCoverBusy] = useState(false);
  const [picking, setPicking] = useState(false);
  const [packageImageError, setPackageImageError] = useState<string | null>(null);
  const [duration, setDuration] = useState<
    { archiveId: string; seconds: number } | { archiveId: string; error: string } | null
  >(null);
  const isPdf = archive.format === "pdf";
  const isBook = isEbook(archive.format);
  const isVideoFile = isVideo(archive.format);

  useEffect(() => {
    if (!isVideoFile) return;
    let active = true;
    const archiveId = archive.archiveId;
    setDuration(null);
    getVideoDuration(archiveId)
      .then((seconds) => {
        if (active) setDuration({ archiveId, seconds });
      })
      .catch((error: unknown) => {
        if (active) setDuration({ archiveId, error: String(error) });
      });
    return () => {
      active = false;
    };
  }, [archive.archiveId, isVideoFile]);

  async function regenerate(entryName: string | null) {
    setPicking(false);
    setCoverBusy(true);
    try {
      await onRegenerateCover(entryName);
    } finally {
      setCoverBusy(false);
    }
  }

  async function choosePackageImage() {
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        title: "パッケージ画像を選択",
        filters: [
          {
            name: "画像",
            extensions: ["png", "webp", "jpg", "jpeg", "gif", "bmp"],
          },
        ],
      });
      if (typeof selected !== "string") return;
      setPackageImageError(null);
      setCoverBusy(true);
      try {
        await onSetPackageImage(selected);
      } finally {
        setCoverBusy(false);
      }
    } catch (reason) {
      setPackageImageError(`パッケージ画像を設定できませんでした: ${String(reason)}`);
    }
  }

  const [category, setCategory] = useState<TagCategory>("author");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !picking) {
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose, picking]);

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
    <>
      <aside
        aria-label="作品の詳細"
        className="fixed bottom-0 right-0 top-[calc(3rem+3.5rem)] z-40 flex w-80 flex-col overflow-hidden border-l border-white/[0.07] bg-black/90 shadow-2xl backdrop-blur-md"
      >
        <div className="flex shrink-0 items-center justify-between p-5 pb-4">
          <h2 className="text-sm font-semibold text-slate-100">作品の詳細</h2>
          <button
            aria-label="閉じる"
            className="grid size-8 place-items-center rounded-full text-sm text-slate-300 transition hover:bg-white/[0.08] focus-visible:outline focus-visible:outline-2 focus-visible:outline-violet-300"
            onClick={onClose}
            title="閉じる (Esc)"
            type="button"
          >
            ✕
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-28">
          <div className="relative mb-4 min-h-12">
            {archive.hasThumbnail ? (
              <img
                alt=""
                className="w-full rounded-xl border border-white/[0.06] object-cover"
                src={thumbnailUrl(archive.archiveId)}
              />
            ) : (
              <img
                alt="サムネイル未指定"
                className="w-full rounded-xl border border-white/[0.06] object-cover"
                src={thumbnailUnavailable}
              />
            )}
          </div>
          <p className="break-all text-sm font-medium leading-6 text-slate-100">
            {archive.filename}
          </p>
          <dl className="mt-3 space-y-1 text-xs text-slate-500">
            <div className="flex flex-col gap-1">
              <dt>ハッシュ値 (SHA-256)</dt>
              <dd className="break-all font-mono text-[10px] leading-4 text-slate-300">
                {archive.archiveId}
              </dd>
            </div>
            <div className="flex justify-between">
              <dt>形式</dt>
              <dd className="uppercase text-slate-300">{archive.format}</dd>
            </div>
            <div className="flex justify-between">
              <dt>{isVideoFile || isBook ? "種別" : "ページ数"}</dt>
              <dd className="text-slate-300">
                {isVideoFile
                  ? "動画"
                  : isBook
                    ? "電子書籍"
                    : archive.pageCount === null
                      ? "不明"
                      : `${archive.pageCount} ページ`}
              </dd>
            </div>
            {isVideoFile && (
              <div className="flex justify-between gap-2">
                <dt>再生時間</dt>
                <dd className="text-right text-slate-300">
                  {duration?.archiveId !== archive.archiveId
                    ? "取得中…"
                    : "error" in duration
                      ? <span title={duration.error}>取得できません（{duration.error}）</span>
                      : formatDuration(duration.seconds)}
                </dd>
              </div>
            )}
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
            <div className="flex justify-between">
              <dt>{isVideoFile ? "最終再生日時" : "最終読書日時"}</dt>
              <dd className="text-slate-300">
                {archive.lastReadAt
                  ? new Date(archive.lastReadAt).toLocaleString("ja-JP")
                  : "未読"}
              </dd>
            </div>
            <div className="flex justify-between">
              <dt>{isVideoFile ? "再生回数" : "読書回数"}</dt>
              <dd className="text-slate-300">{archive.readCount} 回</dd>
            </div>
          </dl>

          <button
            className="mt-5 w-full rounded-lg bg-violet-500/80 px-3 py-2 text-xs font-medium text-white transition hover:bg-violet-500"
            onClick={onOpen}
            type="button"
          >
            ファイルを開く
          </button>

          <button
            aria-pressed={archive.isFavorite}
            className={`mt-2 rounded-lg border px-3 py-2 text-xs font-medium transition ${
              archive.isFavorite
                ? "border-rose-300/30 bg-rose-300/10 text-rose-200"
                : "border-white/[0.08] text-slate-300 hover:bg-white/[0.05]"
            }`}
            onClick={onToggleFavorite}
            type="button"
          >
            {archive.isFavorite ? "♥ お気に入り済み" : "♡ お気に入りに追加"}
          </button>

          <button
            className="mt-2 w-full rounded-lg border border-amber-300/20 px-3 py-2 text-xs font-medium text-amber-100 transition hover:bg-amber-300/[0.08] disabled:opacity-50"
            disabled={unmanageBusy}
            onClick={onUnmanage}
            type="button"
          >
            {unmanageBusy ? "移動中…" : "管理対象外にする（ダウンロードへ移動）"}
          </button>

          {isVideoFile ? (
            <>
              <button
                className="mt-2 w-full rounded-lg border border-white/[0.08] px-2 py-2 text-xs font-medium text-white transition hover:bg-white/[0.05] disabled:opacity-50"
                disabled={coverBusy}
                onClick={() => void choosePackageImage()}
                type="button"
              >
                {coverBusy ? "設定中…" : "パッケージ画像を選ぶ"}
              </button>
              {packageImageError && (
                <p className="mt-2 text-xs text-rose-300" role="alert">
                  {packageImageError}
                </p>
              )}
            </>
          ) : (
            <div className={`mt-2 flex gap-2 ${isPdf || isBook ? "flex-col" : ""}`}>
              <button
                className={`${isPdf || isBook ? "w-full" : "flex-1"} rounded-lg border border-white/[0.08] px-2 py-2 text-xs font-medium text-white transition hover:bg-white/[0.05] disabled:opacity-50`}
                disabled={coverBusy}
                onClick={() => void regenerate(null)}
                title="自動抽出の規則で表紙を作り直します"
                type="button"
              >
                {coverBusy ? "作成中…" : "表紙を再作成"}
              </button>
              {!isPdf && !isBook && (
                <button
                  className="flex-1 rounded-lg border border-white/[0.08] px-2 py-2 text-xs font-medium text-white transition hover:bg-white/[0.05] disabled:opacity-50"
                  disabled={coverBusy}
                  onClick={() => setPicking(true)}
                  title="アーカイブ内の画像から表紙を手動で選びます"
                  type="button"
                >
                  表紙を選ぶ
                </button>
              )}
            </div>
          )}
          {isConvertible(archive.format) && (
            <button
              className="mt-2 w-full rounded-lg border border-white/[0.08] px-2 py-2 text-xs font-medium text-white transition hover:bg-white/[0.05] disabled:opacity-50"
              disabled={coverBusy}
              onClick={onConvert}
              title="PNG/JPEGをAVIFへ変換し、7zで再アーカイブします"
              type="button"
            >
              7z + AVIF へ変換
            </button>
          )}
          <h3 className="mb-2 mt-6 text-xs font-semibold text-slate-400">タグ</h3>
          {archive.tags.length === 0 ? (
            <p className="text-xs text-slate-600">タグはまだありません。</p>
          ) : (
            <ul className="flex flex-wrap gap-1.5">
              {archive.tags.map((tag) => (
                <li
                  className={`flex items-center gap-1 rounded-full border py-1 pr-1 text-[11px] text-slate-200 ${
                    activeTagIds.includes(tag.id)
                      ? "border-violet-300/40 bg-violet-400/15"
                      : "border-white/[0.08] bg-white/[0.04]"
                  }`}
                  key={tag.id}
                >
                  <button
                    aria-pressed={activeTagIds.includes(tag.id)}
                    className="flex items-center gap-1 rounded-full pl-2.5 hover:text-white"
                    onClick={() => onToggleTagFilter(tag.id)}
                    title="左ペインのタグ絞り込みを切り替え"
                    type="button"
                  >
                    <span className="text-slate-500">
                      {tagCategories.find((item) => item.value === tag.category)?.label}
                    </span>
                    {tag.name}
                  </button>
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
              <div className="relative">
                <select
                  aria-label="タグの分類"
                  className="w-full appearance-none rounded-lg border border-white/[0.07] bg-slate-800 py-2 pl-3 pr-7 text-xs text-slate-100 outline-none transition focus:border-violet-300/50"
                  onChange={(event) => setCategory(event.target.value as TagCategory)}
                  value={category}
                >
                  {tagCategories.map((item) => (
                    <option key={item.value} value={item.value}>
                      {item.label}
                    </option>
                  ))}
                </select>
                <span aria-hidden="true" className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 text-[10px] text-slate-300">▾</span>
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
              disabled={busy || !name.trim()}
              type="submit"
            >
              タグを追加
            </button>
          </form>
        </div>
      </aside>
      {picking && (
        <CoverPicker
          archiveId={archive.archiveId}
          onClose={() => setPicking(false)}
          onSelect={(entryName) => void regenerate(entryName)}
        />
      )}
    </>
  );
}

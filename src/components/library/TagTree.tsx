import {
  DragEvent,
  FormEvent,
  KeyboardEvent,
  useMemo,
  useRef,
  useState,
} from "react";
import { TagCategory, TagGroup, TagWithCount } from "../../lib/library";

type Props = {
  category: TagCategory;
  tags: TagWithCount[];
  groups: TagGroup[];
  selectedTagIds: number[];
  onToggleTag: (tagId: number) => void;
  onToggleGroup: (tagIds: number[]) => void;
  onMoveTag: (tagId: number, parentId: number | null) => void;
  onMoveGroup: (groupId: number, parentId: number | null) => void;
  onRenameGroup: (groupId: number, name: string) => void;
  onDeleteGroup: (groupId: number) => void;
  onCreateGroup: (name: string, parentId: number | null) => void;
};

type DragNode = { type: "tag" | "group"; id: number; category: TagCategory };
const dragType = "application/x-fairy-comic-library-node";

type GroupNameInputProps = {
  label: string;
  initialValue?: string;
  placeholder?: string;
  submitLabel?: string;
  asForm?: boolean;
  onSubmit: (name: string) => void;
  onCancel: () => void;
};

function GroupNameInput({
  label,
  initialValue = "",
  placeholder,
  submitLabel,
  asForm = false,
  onSubmit,
  onCancel,
}: GroupNameInputProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const isComposingRef = useRef(false);

  function submit() {
    const name = inputRef.current?.value ?? "";
    if (name.trim()) onSubmit(name);
  }

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    submit();
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onCancel();
      return;
    }
    if (event.key !== "Enter") return;
    if (
      isComposingRef.current ||
      event.nativeEvent.isComposing ||
      event.nativeEvent.keyCode === 229
    ) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (!asForm) {
      event.preventDefault();
      submit();
    }
  }

  const input = (
    <input
      aria-label={label}
      autoFocus
      className={`min-w-0 flex-1 rounded border border-white/10 bg-slate-900 ${
        asForm ? "px-2 text-[11px]" : "px-1 text-xs"
      } py-1 text-slate-100 outline-none focus:border-violet-300/50`}
      autoCapitalize="off"
      autoComplete="off"
      autoCorrect="off"
      maxLength={100}
      defaultValue={initialValue}
      ref={inputRef}
      spellCheck={false}
      type="search"
      onCompositionStart={() => {
        isComposingRef.current = true;
      }}
      onCompositionEnd={() => {
        isComposingRef.current = false;
      }}
      onKeyDown={handleKeyDown}
      placeholder={placeholder}
    />
  );

  if (!asForm) {
    return (
      <>
        {input}
        {submitLabel && (
          <button
            aria-label="グループ名を保存"
            className="size-5 shrink-0 text-xs text-violet-200"
            onClick={submit}
            type="button"
          >
            {submitLabel}
          </button>
        )}
      </>
    );
  }
  return (
    <form className="flex gap-1 py-1 pl-7" onSubmit={handleSubmit}>
      {input}
      <button
        className="rounded px-1.5 text-[10px] text-violet-200 hover:bg-white/10"
        type="submit"
      >
        {submitLabel}
      </button>
      <button
        aria-label="キャンセル"
        className="rounded px-1.5 text-[10px] text-slate-500 hover:bg-white/10"
        onClick={onCancel}
        type="button"
      >
        ×
      </button>
    </form>
  );
}

function readDragNode(dataTransfer: DataTransfer): DragNode | null {
  try {
    const node = JSON.parse(dataTransfer.getData(dragType)) as DragNode;
    return (node.type === "tag" || node.type === "group") &&
      Number.isInteger(node.id) &&
      ["author", "series", "classification"].includes(node.category)
      ? node
      : null;
  } catch {
    return null;
  }
}

export function TagTree({
  category,
  tags,
  groups,
  selectedTagIds,
  onToggleTag,
  onToggleGroup,
  onMoveTag,
  onMoveGroup,
  onRenameGroup,
  onDeleteGroup,
  onCreateGroup,
}: Props) {
  const [expanded, setExpanded] = useState<Set<number>>(() => new Set());
  const [dragging, setDragging] = useState(false);
  const [creatingParent, setCreatingParent] = useState<number | null | undefined>(
    undefined,
  );
  const [editingGroupId, setEditingGroupId] = useState<number | null>(null);
  const filteredTags = useMemo(
    () => tags.filter((tag) => tag.category === category),
    [category, tags],
  );
  const filteredGroups = useMemo(
    () => groups.filter((group) => group.category === category),
    [category, groups],
  );
  const childrenByGroup = useMemo(() => {
    const children = new Map<number | null, TagGroup[]>();
    for (const group of filteredGroups) {
      children.set(group.parentId, [...(children.get(group.parentId) ?? []), group]);
    }
    for (const list of children.values()) {
      list.sort((a, b) => a.name.localeCompare(b.name, "ja"));
    }
    return children;
  }, [filteredGroups]);
  const tagsByGroup = useMemo(() => {
    const children = new Map<number | null, TagWithCount[]>();
    for (const tag of filteredTags) {
      children.set(tag.groupId, [...(children.get(tag.groupId) ?? []), tag]);
    }
    for (const list of children.values()) {
      list.sort((a, b) => a.name.localeCompare(b.name, "ja"));
    }
    return children;
  }, [filteredTags]);

  function descendantTags(groupId: number): TagWithCount[] {
    const nested = childrenByGroup.get(groupId) ?? [];
    return [
      ...(tagsByGroup.get(groupId) ?? []),
      ...nested.flatMap((group) => descendantTags(group.id)),
    ];
  }

  function startDrag(event: DragEvent, node: DragNode) {
    event.dataTransfer.setData(dragType, JSON.stringify(node));
    event.dataTransfer.effectAllowed = "move";
    setDragging(true);
  }

  function dropOn(event: DragEvent, parentId: number | null) {
    event.preventDefault();
    event.stopPropagation();
    setDragging(false);
    const node = readDragNode(event.dataTransfer);
    if (!node || node.category !== category) return;
    if (node.type === "tag") onMoveTag(node.id, parentId);
    else onMoveGroup(node.id, parentId);
  }

  function groupForm(parentId: number | null) {
    if (creatingParent !== parentId) return null;
    return (
      <GroupNameInput
        key={`create-${parentId ?? "root"}`}
        label="新しいグループ名"
        placeholder="グループ名"
        submitLabel="追加"
        asForm
        onCancel={() => setCreatingParent(undefined)}
        onSubmit={(name) => {
          onCreateGroup(name, parentId);
          setCreatingParent(undefined);
        }}
      />
    );
  }

  function renderGroup(group: TagGroup, depth: number) {
    const descendants = descendantTags(group.id);
    const selectedCount = descendants.filter((tag) =>
      selectedTagIds.includes(tag.id),
    ).length;
    const isExpanded = expanded.has(group.id);
    return (
      <div key={group.id}>
        <div
          className="group flex items-center gap-1 rounded-lg pr-1 hover:bg-white/[0.04]"
          draggable
          onDragEnd={() => setDragging(false)}
          onDragOver={(event) => {
            if (event.dataTransfer.types.includes(dragType)) event.preventDefault();
          }}
          onDrop={(event) => dropOn(event, group.id)}
          onDragStart={(event) =>
            startDrag(event, { type: "group", id: group.id, category })
          }
          style={{
            minWidth: `${depth * 12 + 164}px`,
            paddingLeft: `${depth * 12 + 4}px`,
          }}
        >
          <button
            aria-label={`${group.name}を${isExpanded ? "折りたたむ" : "展開"}`}
            aria-expanded={isExpanded}
            className="grid size-5 shrink-0 place-items-center text-base leading-none text-slate-300"
            onClick={() =>
              setExpanded((current) => {
                const next = new Set(current);
                if (next.has(group.id)) next.delete(group.id);
                else next.add(group.id);
                return next;
              })
            }
            type="button"
          >
            {isExpanded ? "⌄" : "›"}
          </button>
          {editingGroupId === group.id ? (
            <GroupNameInput
              key={`rename-${group.id}`}
              label="グループ名を変更"
              initialValue={group.name}
              submitLabel="✓"
              onCancel={() => setEditingGroupId(null)}
              onSubmit={(name) => {
                onRenameGroup(group.id, name);
                setEditingGroupId(null);
              }}
            />
          ) : (
            <button
              aria-pressed={
                selectedCount === 0
                  ? false
                  : selectedCount === descendants.length
                    ? true
                    : "mixed"
              }
              className={`min-w-[2rem] flex-1 truncate py-1.5 text-left text-xs ${
                selectedCount === descendants.length && descendants.length > 0
                  ? "text-violet-100"
                  : selectedCount > 0
                    ? "text-violet-200"
                    : "text-slate-400 hover:text-slate-200"
              }`}
              onClick={() => onToggleGroup(descendants.map((tag) => tag.id))}
              title={group.name}
              type="button"
            >
              {group.name}
            </button>
          )}
          <span className="shrink-0 text-[10px] text-slate-600">
            {selectedCount > 0 ? `${selectedCount}/` : ""}
            {descendants.length}
          </span>
          <button
            aria-label={`${group.name}の下にグループを追加`}
            className="invisible grid size-5 shrink-0 place-items-center text-xs text-slate-500 hover:text-violet-200 group-hover:visible group-focus-within:visible"
            onClick={() => {
              setCreatingParent(group.id);
              setExpanded((current) => new Set(current).add(group.id));
            }}
            type="button"
          >
            +
          </button>
          <button
            aria-label={`グループ ${group.name} の名前を変更`}
            className="invisible grid size-5 shrink-0 place-items-center text-xs text-slate-500 hover:text-slate-200 group-hover:visible group-focus-within:visible"
            onClick={() => {
              setEditingGroupId(group.id);
            }}
            type="button"
          >
            ✎
          </button>
          <button
            aria-label={`グループ ${group.name} を削除`}
            className="invisible grid size-5 shrink-0 place-items-center text-xs text-slate-500 hover:text-rose-300 group-hover:visible group-focus-within:visible"
            onClick={() => onDeleteGroup(group.id)}
            type="button"
          >
            ×
          </button>
        </div>
        {isExpanded && (
          <div>
            {(childrenByGroup.get(group.id) ?? []).map((child) =>
              renderGroup(child, depth + 1),
            )}
            {(tagsByGroup.get(group.id) ?? []).map(renderTag)}
            {groupForm(group.id)}
          </div>
        )}
      </div>
    );
  }

  function renderTag(tag: TagWithCount) {
    const selected = selectedTagIds.includes(tag.id);
    return (
      <button
        aria-pressed={selected}
        className={`flex w-full items-center justify-between gap-2 rounded-lg py-1.5 pr-2 text-left text-xs transition ${
          selected
            ? "bg-violet-400/15 text-violet-100"
            : "text-slate-400 hover:bg-white/[0.04] hover:text-slate-200"
        }`}
        draggable
        key={tag.id}
        onClick={() => onToggleTag(tag.id)}
        onDragEnd={() => setDragging(false)}
        onDragStart={(event) =>
          startDrag(event, { type: "tag", id: tag.id, category })
        }
        style={{ paddingLeft: `${(tag.groupId === null ? 0 : getDepth(tag.groupId)) * 12 + 28}px` }}
        type="button"
      >
        <span className="truncate">{tag.name}</span>
        <span className="shrink-0 text-[10px] text-slate-600">{tag.archiveCount}</span>
      </button>
    );
  }

  function getDepth(groupId: number): number {
    const group = filteredGroups.find((item) => item.id === groupId);
    return group ? 1 + (group.parentId === null ? 0 : getDepth(group.parentId)) : 0;
  }

  return (
    <div className="rounded-lg">
      <div className="mb-1 flex items-center justify-between">
        <span className="px-3 text-[10px] text-slate-600">タグとグループ</span>
        <button
          className="rounded px-2 py-1 text-[10px] text-violet-200/80 hover:bg-white/[0.05]"
          onClick={() => {
            setCreatingParent(null);
          }}
          type="button"
        >
          ＋ グループ
        </button>
      </div>
      <div
        className={`space-y-0.5 overflow-x-auto rounded-lg ${dragging ? "min-h-6" : ""}`}
        onDragOver={(event) => {
          if (event.dataTransfer.types.includes(dragType)) event.preventDefault();
        }}
        onDrop={(event) => dropOn(event, null)}
      >
        {(childrenByGroup.get(null) ?? []).map((group) => renderGroup(group, 0))}
        {(tagsByGroup.get(null) ?? []).map(renderTag)}
        {groupForm(null)}
        {dragging && (
          <div className="rounded border border-dashed border-violet-300/30 px-2 py-1 text-[10px] text-violet-200/70">
            ルートへ移動
          </div>
        )}
      </div>
    </div>
  );
}

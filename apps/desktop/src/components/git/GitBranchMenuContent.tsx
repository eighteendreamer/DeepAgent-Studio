import type { IconProp } from "@fortawesome/fontawesome-svg-core";

import { useMemo } from "react";

import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

import { useTranslation } from "react-i18next";

import type { TFunction } from "i18next";

import type { GitBranch } from "../../types";

import { Panel } from "../ui/Panel";

import { ListItem } from "../ui/ListItem";

import { MENU_LIST } from "../ui/motion";

import { MENU_ITEM_ATTR, SlidingMenuList } from "../ui/SlidingMenuList";

import { cn } from "../shadcn/utils";

/** Git 分支菜单 —— 与摘要卡片同宽同对齐（px-2 外框 + px-2.5 内容列） */
const GIT_MENU = {
  padX: "px-2",
  contentX: "px-2.5",
  divider: "mx-2 my-1.5 h-px shrink-0 bg-border-theme opacity-[0.55]",
  searchWrap: "px-2 pb-2.5 pt-3",
  searchBar: "flex items-center text-[13px] text-text-secondary",
  icon: "mr-2 w-3.5 shrink-0 text-[13px] text-text-secondary",
  summaryCard: "flex items-start justify-between gap-2 rounded-lg bg-black/5 px-2.5 py-2",
  section: "flex items-center px-2.5 pb-0.5 pt-1.5 text-[10px] font-medium text-text-secondary",
  row: "flex w-full items-start justify-between gap-2 rounded-lg px-2.5 py-2 text-left text-[12px]",
  rowCompact: "flex w-full items-start justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-[11px]",
  pill: "left-0 right-0 rounded-lg",
} as const;



interface GitBranchMenuContentProps {

  branches: GitBranch[];

  loading: boolean;

  busy: string | null;

  query: string;

  onQueryChange: (query: string) => void;

  currentBranch: string | null;

  additions: number;

  deletions: number;

  filesChanged: number;

  rebaseState: string | null;

  mergeState: boolean;

  operationError: string | null;

  operationResult: string | null;

  onCheckout: (branch: GitBranch) => void;

  onFetch: () => void;

  onCreateBranch: () => void;

  onOpenWorkbench?: () => void;

  className?: string;

  compact?: boolean;

  small?: boolean;

  /** 嵌套在 DropdownMenuContent 内时为 true，避免双层阴影/动效 */
  embedded?: boolean;

}



export function GitBranchMenuContent({

  branches,

  loading,

  busy,

  query,

  onQueryChange,

  currentBranch,

  additions,

  deletions,

  filesChanged,

  rebaseState,

  mergeState,

  operationError,

  operationResult,

  onCheckout,

  onFetch,

  onCreateBranch,

  onOpenWorkbench,

  className = "",

  compact = false,

  small = false,

  embedded = false,

}: GitBranchMenuContentProps) {

  const { t } = useTranslation();

  const filtered = useMemo(() => {

    const q = query.trim().toLowerCase();

    if (!q) return branches;

    return branches.filter(

      (branch) =>

        branch.name.toLowerCase().includes(q) ||

        branch.subject?.toLowerCase().includes(q) ||

        branch.upstream?.toLowerCase().includes(q),

    );

  }, [branches, query]);



  const local = filtered.filter((branch) => branch.kind === "local");

  const remote = filtered.filter((branch) => branch.kind === "remote");



  const widthClass = small ? "w-[240px]" : compact ? "w-[260px]" : "w-[380px]";

  const listHeightClass = small ? "max-h-[120px]" : compact ? "max-h-[160px]" : "max-h-[280px]";

  const rowClass = small ? GIT_MENU.rowCompact : GIT_MENU.row;

  const textSizeClass = small ? "text-[11px]" : "text-[12px]";

  const subTextClass = small ? "text-[10px]" : "text-[11px]";

  const menuRow = (extra?: string) => cn(rowClass, "text-text-base", extra);

  return (

    <Panel

      menu={!embedded}

      className={cn(`${widthClass} max-w-[calc(100vw-48px)] overflow-hidden rounded-2xl shadow-none`, className)}

    >

      <div className={cn(GIT_MENU.searchWrap, GIT_MENU.searchBar)}>

        <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className={GIT_MENU.icon} />

        <input

          value={query}

          onChange={(e) => onQueryChange(e.target.value)}

          onKeyDown={(e) => e.stopPropagation()}

          placeholder={t("git.searchBranches")}

          className={MENU_LIST.searchInput}

          autoFocus

        />

      </div>



      <div className={GIT_MENU.divider} aria-hidden />



      <div className={cn(GIT_MENU.padX, "pb-2")}>

        <div className={GIT_MENU.summaryCard}>

          <div className="min-w-0">

            <div className={cn("flex items-center font-medium text-text-base", textSizeClass)}>

              <FontAwesomeIcon icon={["fas", "code-branch"]} className={GIT_MENU.icon} />

              <span className="truncate">{currentBranch ?? t("git.detachedHead")}</span>

              {(rebaseState || mergeState) && (

                <span className="ml-2 rounded bg-amber-100 px-1.5 py-0.5 text-[10px] text-amber-700">

                  {rebaseState ? t("git.rebasing") : t("git.merging")}

                </span>

              )}

            </div>

            <div className={cn("mt-0.5 truncate text-text-secondary", subTextClass)}>

              {filesChanged > 0 ? t("git.changedFilesCount", { count: filesChanged }) : t("git.workingTreeClean")}

            </div>

          </div>

          <div className="ml-2 flex shrink-0 items-center gap-1.5 text-[11px] font-medium tabular-nums">

            <span className="text-green-600">+{additions}</span>

            <span className="text-red-500">-{deletions}</span>

          </div>

        </div>

        {operationError && <Message tone="error">{operationError}</Message>}

        {operationResult && <Message tone="success">{operationResult}</Message>}

      </div>



      <div className={cn(GIT_MENU.padX, listHeightClass, "overflow-y-auto pb-0.5")}>

      <SlidingMenuList
        activeId={currentBranch ?? ""}
        pillClassName={GIT_MENU.pill}
        className="w-full"
      >

        {loading && <div className={cn(GIT_MENU.contentX, "py-2 text-[12px] text-text-secondary")}>{t("git.loadingState")}</div>}

        {!loading && branches.length === 0 && (

          <div className={cn(GIT_MENU.contentX, "py-2 text-[12px] text-text-secondary")}>{t("git.noBranchesFound")}</div>

        )}

        <BranchSection

          title={t("git.localBranches")}

          icon={["fas", "folder"]}

          branches={local}

          busy={busy}

          onCheckout={onCheckout}

          rowClass={rowClass}

          subTextClass={subTextClass}

        />

        <BranchSection

          title={t("git.remoteBranches")}

          icon={["fas", "cloud"]}

          branches={remote}

          busy={busy}

          onCheckout={onCheckout}

          rowClass={rowClass}

          subTextClass={subTextClass}

        />

      </SlidingMenuList>

      </div>



      <div className={GIT_MENU.divider} aria-hidden />



      <div className={cn(GIT_MENU.padX, "pb-2 pt-0.5")}>

      <SlidingMenuList activeId="" pillClassName={GIT_MENU.pill} className="w-full">

        <ListItem

          {...{ [MENU_ITEM_ATTR]: "fetch" }}

          sliding

          className={menuRow(busy ? "cursor-default text-text-secondary" : "cursor-pointer")}

          onClick={busy ? undefined : onFetch}

        >

          <span className="flex min-w-0 items-center">

            <FontAwesomeIcon icon={["fas", "rotate-right"]} className={GIT_MENU.icon} />

            {busy === "fetch" ? t("git.fetching") : t("git.fetch")}

          </span>

        </ListItem>

        <ListItem

          {...{ [MENU_ITEM_ATTR]: "create" }}

          sliding

          className={menuRow(busy ? "cursor-default text-text-secondary" : "cursor-pointer")}

          onClick={busy ? undefined : onCreateBranch}

        >

          <span className="flex min-w-0 items-center">

            <FontAwesomeIcon icon={["fas", "plus"]} className={GIT_MENU.icon} />

            {t("git.createAndCheckoutBranch")}

          </span>

        </ListItem>

        <ListItem

          {...{ [MENU_ITEM_ATTR]: "open-git" }}

          sliding

          className={menuRow(

            onOpenWorkbench ? "cursor-pointer text-text-base" : "cursor-default text-text-secondary",

          )}

          title={onOpenWorkbench ? t("git.openGit") : t("git.openGitUnavailable")}

          onClick={onOpenWorkbench}

        >

          <span className="flex min-w-0 items-center">

            <FontAwesomeIcon icon={["fas", "table"]} className={GIT_MENU.icon} />

            {t("git.openGit")}

          </span>

        </ListItem>

      </SlidingMenuList>

      </div>

    </Panel>

  );

}



function BranchSection({

  title,

  icon,

  branches,

  busy,

  onCheckout,

  rowClass,

  subTextClass,

}: {

  title: string;

  icon: IconProp;

  branches: GitBranch[];

  busy: string | null;

  onCheckout: (branch: GitBranch) => void;

  rowClass: string;

  subTextClass: string;

}) {

  const { t } = useTranslation();



  if (branches.length === 0) return null;

  return (

    <div>

      <div className={GIT_MENU.section}>

        <FontAwesomeIcon icon={icon} className={GIT_MENU.icon} />

        <span>{title}</span>

      </div>

      {branches.map((branch) => {

        const disabled = !!busy || branch.current || !!branch.worktree_path;

        return (

          <ListItem

            key={branch.full_name}

            {...{ [MENU_ITEM_ATTR]: branch.name }}

            sliding

            selected={branch.current}

            className={cn(

              rowClass,

              "items-start text-text-base",

              disabled ? "cursor-default" : "cursor-pointer",

            )}

            title={branch.worktree_path ? t("git.worktreeAt", { path: branch.worktree_path }) : branch.subject ?? branch.name}

            onClick={disabled ? undefined : () => onCheckout(branch)}

          >

            <div className="flex min-w-0 items-start">

              <FontAwesomeIcon icon={["fas", "code-branch"]} className={cn(GIT_MENU.icon, "mt-0.5")} />

              <div className="min-w-0">

                <div className="truncate font-medium">{branch.name}</div>

                <div className={cn(subTextClass, "truncate text-text-secondary")}>

                  {branch.worktree_path ? t("git.worktreeAt", { path: branch.worktree_path }) : branch.subject ?? branch.upstream ?? ""}

                </div>

              </div>

            </div>

            <div className="flex shrink-0 items-center gap-2 pt-0.5">

              {(branch.ahead > 0 || branch.behind > 0) && (

                <span className={cn(subTextClass, "font-medium text-blue-600")}>

                  {formatAheadBehind(branch.ahead, branch.behind, t)}

                </span>

              )}

              {branch.current && <FontAwesomeIcon icon={["fas", "check"]} className="text-[11px] text-text-base" />}

            </div>

          </ListItem>

        );

      })}

    </div>

  );

}



function Message({ children, tone }: { children: string; tone: "error" | "success" }) {

  const cls = tone === "error" ? "bg-red-50 text-red-600" : "bg-green-50 text-green-700";

  return <div className={cn("mt-2 rounded-lg px-2.5 py-2 text-[12px]", cls)}>{children}</div>;

}



export function formatAheadBehind(ahead: number, behind: number, t: TFunction): string {

  return [

    ahead > 0 ? t("git.aheadCount", { count: ahead }) : "",

    behind > 0 ? t("git.behindCount", { count: behind }) : "",

  ]

    .filter(Boolean)

    .join(" ");

}



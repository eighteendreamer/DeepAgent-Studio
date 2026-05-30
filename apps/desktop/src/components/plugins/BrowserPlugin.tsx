import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

export function BrowserPlugin() {
  return (
    <div className="w-full h-full flex flex-col bg-white">
      {/* Toolbar */}
      <div className="flex items-center justify-between px-4 py-2 border-b border-border-theme flex-shrink-0">
        <div className="flex items-center space-x-4 text-text-secondary">
          <FontAwesomeIcon icon={["fas", "arrow-left"]} className="cursor-pointer hover:text-text-base" />
          <FontAwesomeIcon icon={["fas", "arrow-right"]} className="cursor-not-allowed opacity-50" />
          <FontAwesomeIcon icon={["fas", "rotate-right"]} className="cursor-pointer hover:text-text-base text-[13px]" />
        </div>
        <div className="flex-1 flex justify-center text-[13px] text-text-base">
          baidu.com
        </div>
        <div className="flex items-center space-x-4 text-text-secondary">
          <FontAwesomeIcon icon={["fas", "expand"]} className="cursor-pointer hover:text-text-base text-[13px]" />
          <FontAwesomeIcon icon={["fas", "ellipsis"]} className="cursor-pointer hover:text-text-base" />
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 bg-white">
        {/* Placeholder for iframe */}
      </div>
    </div>
  );
}

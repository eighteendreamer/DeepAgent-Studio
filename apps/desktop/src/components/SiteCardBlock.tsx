import { memo } from "react";

interface SiteCardProps {
  content: string;
}

export const SiteCardBlock = memo(function SiteCardBlock({ content }: SiteCardProps) {
  let data: any;
  try {
    data = JSON.parse(content.trim());
  } catch (e) {
    return (
      <div className="my-2 rounded border border-red-500/50 bg-red-500/10 p-2 text-sm text-red-500">
        网站详情解析失败：无效的 JSON 格式
      </div>
    );
  }

  return (
    <div className="my-3 flex w-full max-w-[450px] flex-col overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
      {/* 封面图区域 */}
      {data.coverImage && (
        <div className="relative w-full cursor-pointer bg-gray-100 dark:bg-gray-900 overflow-hidden">
          <img
            src={data.coverImage}
            alt="cover"
            className="w-full object-cover max-h-[240px] hover:scale-105 transition-transform duration-300"
          />
          {/* 如果是视频，显示居中的播放按钮 */}
          {data.isVideo && (
            <div className="absolute inset-0 flex items-center justify-center bg-black/10">
              <div className="flex h-12 w-12 items-center justify-center rounded-full bg-black/60 text-white backdrop-blur-md transition-transform hover:scale-110">
                <svg width="24" height="24" viewBox="0 0 24 24" fill="currentColor">
                  <path d="M8 5v14l11-7z" />
                </svg>
              </div>
            </div>
          )}
        </div>
      )}

      {/* 站点头部信息 */}
      <div className="flex items-center justify-between px-4 py-3">
        <div className="flex items-center gap-2">
          {data.icon ? (
            <img src={data.icon} alt="icon" className="h-5 w-5 rounded object-cover" />
          ) : (
            <div className="flex h-5 w-5 items-center justify-center rounded bg-gray-200 dark:bg-gray-700">
              <span className="text-[10px] text-gray-500 dark:text-gray-400">🌐</span>
            </div>
          )}
          <span className="text-[14px] font-medium text-gray-800 dark:text-gray-200">
            {data.siteName || "网站链接"}
          </span>
        </div>
        <div className="cursor-pointer text-gray-400 hover:text-gray-600 dark:hover:text-gray-300">
          <svg
            width="18"
            height="18"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="12" cy="12" r="1"></circle>
            <circle cx="19" cy="12" r="1"></circle>
            <circle cx="5" cy="12" r="1"></circle>
          </svg>
        </div>
      </div>

      {/* 站点描述文本 */}
      {data.description && (
        <div className="px-4 pb-4 pt-0">
          <p className="text-[13px] leading-relaxed text-gray-600 line-clamp-3 dark:text-gray-400">
            {data.description}
          </p>
        </div>
      )}
    </div>
  );
});

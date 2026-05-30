import { useEffect } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { useTranslation } from "react-i18next";

interface Props {
  isOpen: boolean;
  onClose: () => void;
}

const SEARCH_RESULTS = [
  { title: "你好", folder: "Looker-v2", shortcut: "Ctrl+1" },
  { title: "你好", folder: "DeepAgent-St...", shortcut: "Ctrl+2" },
  { title: "你好", folder: "Looker-v2", shortcut: "Ctrl+3" },
  { title: "你好", folder: "Looker-v2", shortcut: "Ctrl+4" },
  { title: "更新Trinity 模式（Trinity JWT + Trinity 计费）", folder: "Looker-v2", shortcut: "Ctrl+5" },
  { title: "代码部署调试", folder: "Looker-v2", shortcut: "Ctrl+6" },
  { title: "问候", folder: "Looker-v2", shortcut: "Ctrl+7" },
  { title: "问候", folder: "LingLearn_Cloud", shortcut: "Ctrl+8" },
  { title: "CPI Trinity 计费规范", folder: "Looker-v2", shortcut: "Ctrl+9" },
];

export function SearchModal({ isOpen, onClose }: Props) {
  const { t } = useTranslation();
  useEffect(() => {
    if (!isOpen) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, onClose]);

  return (
    <AnimatePresence>
      {isOpen && (
        <motion.div 
          className="fixed inset-0 z-[100] flex items-start justify-center pt-[10vh] bg-transparent"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.15 }}
        >
          {/* Backdrop overlay for closing */}
          <div 
            className="absolute inset-0 bg-black/5" 
            onClick={onClose} 
          />
      
          {/* Modal */}
          <motion.div 
            className="relative w-[600px] max-h-[80vh] bg-white rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] border border-border-theme flex flex-col overflow-hidden"
            initial={{ scale: 0.95, y: -10 }}
            animate={{ scale: 1, y: 0 }}
            exit={{ scale: 0.95, y: -10 }}
            transition={{ type: "spring", bounce: 0, duration: 0.25 }}
          >
        {/* Header / Input area */}
        <div className="px-5 py-4 border-b border-transparent">
          <input 
            type="text" 
            placeholder={t("searchModal.searchChats")}
            autoFocus
            className="w-full text-base bg-transparent outline-none text-text-base placeholder:text-text-secondary placeholder:font-normal"
          />
        </div>

        {/* List Content */}
        <div className="flex-1 overflow-y-auto px-2 pb-2">
          <div className="px-3 py-2 text-xs text-text-secondary font-medium">{t("searchModal.recentChats")}</div>
          <div className="flex flex-col space-y-0.5">
            {SEARCH_RESULTS.map((item, i) => (
              <div 
                key={i}
                className="flex items-center justify-between px-3 py-2.5 rounded-lg cursor-pointer hover:bg-gray-100 group transition-colors"
              >
                <div className="text-[14px] text-text-base truncate pr-4 flex-1">
                  {item.title}
                </div>
                <div className="flex items-center space-x-3 flex-shrink-0">
                  <span className="text-[12px] text-text-secondary truncate max-w-[120px]">
                    {item.folder}
                  </span>
                  <span className="text-[11px] text-gray-400 bg-gray-50 border border-gray-200 rounded px-1.5 py-0.5 font-sans min-w-[42px] text-center group-hover:bg-white transition-colors">
                    {item.shortcut}
                  </span>
                </div>
              </div>
            ))}
          </div>
        </div>
      </motion.div>
    </motion.div>
      )}
    </AnimatePresence>
  );
}

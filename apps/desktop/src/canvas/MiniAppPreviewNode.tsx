import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
  faBatteryFull,
  faChevronRight,
  faClock,
  faCrosshairs,
  faGripVertical,
  faHouse,
  faLocationDot,
  faMagnifyingGlass,
  faMobileScreenButton,
  faMugHot,
  faReceipt,
  faRotateRight,
  faUser,
  faWifi,
} from "@fortawesome/free-solid-svg-icons";
import { useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";

export type MiniAppDeviceId = "iphone-15-pro" | "iphone-se" | "pixel-8" | "ipad-mini";
export type MiniAppOrientation = "portrait" | "landscape";
export type MiniAppRoute = "home" | "orders";

export type MiniAppPreviewConfig = {
  deviceId: MiniAppDeviceId;
  orientation: MiniAppOrientation;
  route: MiniAppRoute;
  inspectorEnabled: boolean;
};

export const DEFAULT_MINIAPP_PREVIEW: MiniAppPreviewConfig = {
  deviceId: "iphone-15-pro",
  orientation: "portrait",
  route: "home",
  inspectorEnabled: false,
};

type DevicePreset = {
  id: MiniAppDeviceId;
  name: string;
  width: number;
  height: number;
  platform: "iOS" | "Android";
};

const DEVICE_PRESETS: DevicePreset[] = [
  { id: "iphone-15-pro", name: "iPhone 15 Pro", width: 393, height: 852, platform: "iOS" },
  { id: "iphone-se", name: "iPhone SE", width: 375, height: 667, platform: "iOS" },
  { id: "pixel-8", name: "Pixel 8", width: 412, height: 915, platform: "Android" },
  { id: "ipad-mini", name: "iPad mini", width: 744, height: 1133, platform: "iOS" },
];

type MiniAppPreviewNodeProps = {
  data: {
    title: string;
    locked?: boolean;
    preview?: MiniAppPreviewConfig;
  };
  selected: boolean;
  toolbar: ReactNode;
  resizer: ReactNode;
  onChange: (patch: Partial<MiniAppPreviewConfig>) => void;
};

function inspectableProps(name: string, selected: string | null) {
  return {
    "data-mini-node": name,
    "data-mini-selected": selected === name ? "true" : undefined,
  };
}

export function MiniAppPreviewNode({
  data,
  selected,
  toolbar,
  resizer,
  onChange,
}: MiniAppPreviewNodeProps) {
  const config = data.preview ?? DEFAULT_MINIAPP_PREVIEW;
  const device = useMemo(
    () => DEVICE_PRESETS.find((candidate) => candidate.id === config.deviceId) ?? DEVICE_PRESETS[0],
    [config.deviceId],
  );
  const [refreshing, setRefreshing] = useState(false);
  const [selectedElement, setSelectedElement] = useState<string | null>(null);
  const refreshTimerRef = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (refreshTimerRef.current !== null) window.clearTimeout(refreshTimerRef.current);
    },
    [],
  );

  useEffect(() => {
    if (!config.inspectorEnabled) setSelectedElement(null);
  }, [config.inspectorEnabled]);

  const refresh = () => {
    setRefreshing(true);
    if (refreshTimerRef.current !== null) window.clearTimeout(refreshTimerRef.current);
    refreshTimerRef.current = window.setTimeout(() => {
      setRefreshing(false);
      refreshTimerRef.current = null;
    }, 650);
  };

  const handlePreviewClick = (event: MouseEvent<HTMLDivElement>) => {
    if (!config.inspectorEnabled) return;
    const element = (event.target as HTMLElement).closest<HTMLElement>("[data-mini-node]");
    if (!element) return;
    event.preventDefault();
    event.stopPropagation();
    setSelectedElement(element.dataset.miniNode ?? null);
  };

  const logicalWidth = config.orientation === "portrait" ? device.width : device.height;
  const logicalHeight = config.orientation === "portrait" ? device.height : device.width;
  const isPortrait = config.orientation === "portrait";

  return (
    <div
      className={`studio-miniapp-node${selected ? " is-selected" : ""}${data.locked ? " is-locked" : ""}`}
    >
      {toolbar}
      {resizer}

      <div className="studio-miniapp-header studio-preview-drag-handle">
        <span className="studio-miniapp-grip" aria-hidden="true">
          <FontAwesomeIcon icon={faGripVertical} />
        </span>
        <span className="studio-miniapp-app-icon">
          <FontAwesomeIcon icon={faMobileScreenButton} />
        </span>
        <span className="studio-miniapp-heading">
          <strong>{data.title || "小程序预览"}</strong>
          <small>示例商城 · 前端原型</small>
        </span>
        <span className="studio-miniapp-runtime-status">
          <i /> 预览中
        </span>
        <div className="studio-miniapp-header-actions nodrag">
          <button
            type="button"
            className={config.inspectorEnabled ? "is-active" : ""}
            title={config.inspectorEnabled ? "退出元素选择" : "选择元素"}
            aria-pressed={config.inspectorEnabled}
            onClick={() => onChange({ inspectorEnabled: !config.inspectorEnabled })}
          >
            <FontAwesomeIcon icon={faCrosshairs} />
          </button>
          <button type="button" title="旋转设备" onClick={() => onChange({ orientation: isPortrait ? "landscape" : "portrait" })}>
            <FontAwesomeIcon icon={faRotateRight} />
          </button>
          <button type="button" title="刷新预览" onClick={refresh}>
            <FontAwesomeIcon icon={faRotateRight} spin={refreshing} />
          </button>
        </div>
      </div>

      <div className="studio-miniapp-controls nodrag nowheel">
        <label>
          <span>设备</span>
          <select
            aria-label="选择预览设备"
            value={config.deviceId}
            onChange={(event) => onChange({ deviceId: event.target.value as MiniAppDeviceId })}
          >
            {DEVICE_PRESETS.map((preset) => (
              <option key={preset.id} value={preset.id}>{preset.name}</option>
            ))}
          </select>
        </label>
        <label>
          <span>页面</span>
          <select
            aria-label="选择小程序页面"
            value={config.route}
            onChange={(event) => onChange({ route: event.target.value as MiniAppRoute })}
          >
            <option value="home">首页</option>
            <option value="orders">我的订单</option>
          </select>
        </label>
        <span className="studio-miniapp-viewport-size">{logicalWidth} × {logicalHeight}</span>
      </div>

      <div className={`studio-miniapp-stage nodrag nowheel${refreshing ? " is-refreshing" : ""}`}>
        <div
          className={`studio-device-shell is-${config.orientation}`}
          style={{ aspectRatio: `${logicalWidth} / ${logicalHeight}` }}
        >
          <div className="studio-device-screen" onClickCapture={handlePreviewClick}>
            <div className="studio-device-statusbar" {...inspectableProps("状态栏", selectedElement)}>
              <span>9:41</span>
              <span><FontAwesomeIcon icon={faWifi} /><FontAwesomeIcon icon={faBatteryFull} /></span>
            </div>

            {config.route === "home" ? (
              <div className="studio-mock-app-page is-home">
                <header {...inspectableProps("首页导航", selectedElement)}>
                  <div>
                    <small>当前位置</small>
                    <strong><FontAwesomeIcon icon={faLocationDot} /> 科技园店</strong>
                  </div>
                  <button type="button" aria-label="用户中心"><FontAwesomeIcon icon={faUser} /></button>
                </header>
                <main>
                  <section className="studio-mock-hero" {...inspectableProps("活动横幅", selectedElement)}>
                    <small>今日限定</small>
                    <h3>让下午慢一点</h3>
                    <p>第二杯半价 · 会员专享</p>
                    <FontAwesomeIcon icon={faMugHot} />
                  </section>

                  <div className="studio-mock-search" {...inspectableProps("搜索框", selectedElement)}>
                    <FontAwesomeIcon icon={faMagnifyingGlass} />
                    <span>搜索饮品或活动</span>
                  </div>

                  <section className="studio-mock-section" {...inspectableProps("推荐商品", selectedElement)}>
                    <div className="studio-mock-section-title"><strong>本周推荐</strong><span>全部 <FontAwesomeIcon icon={faChevronRight} /></span></div>
                    <div className="studio-mock-products">
                      <button type="button" {...inspectableProps("生椰拿铁商品卡", selectedElement)}>
                        <span className="studio-product-art is-coconut"><FontAwesomeIcon icon={faMugHot} /></span>
                        <strong>生椰拿铁</strong><small>清爽椰香</small><b>¥18</b>
                      </button>
                      <button type="button" {...inspectableProps("橙香美式商品卡", selectedElement)}>
                        <span className="studio-product-art is-orange"><FontAwesomeIcon icon={faMugHot} /></span>
                        <strong>橙香美式</strong><small>明亮果香</small><b>¥16</b>
                      </button>
                    </div>
                  </section>
                </main>
              </div>
            ) : (
              <div className="studio-mock-app-page is-orders">
                <header {...inspectableProps("订单页导航", selectedElement)}>
                  <div><small>示例商城</small><strong>我的订单</strong></div>
                  <button type="button" aria-label="搜索订单"><FontAwesomeIcon icon={faMagnifyingGlass} /></button>
                </header>
                <main>
                  <nav className="studio-order-tabs" {...inspectableProps("订单筛选", selectedElement)}>
                    <button className="is-active" type="button">全部</button><button type="button">制作中</button><button type="button">已完成</button>
                  </nav>
                  <section className="studio-order-card" {...inspectableProps("订单卡片", selectedElement)}>
                    <div><span>科技园店</span><small>制作中</small></div>
                    <p><span className="studio-order-thumb"><FontAwesomeIcon icon={faMugHot} /></span><strong>生椰拿铁 × 1</strong><b>¥18</b></p>
                    <footer><FontAwesomeIcon icon={faClock} /> 预计 14:35 完成</footer>
                  </section>
                  <section className="studio-order-empty" {...inspectableProps("历史订单提示", selectedElement)}>
                    <FontAwesomeIcon icon={faReceipt} /><strong>更多订单会显示在这里</strong><small>前端原型暂未连接真实数据</small>
                  </section>
                </main>
              </div>
            )}

            <nav className="studio-mock-tabbar" {...inspectableProps("底部导航", selectedElement)}>
              <button type="button" className={config.route === "home" ? "is-active" : ""} onClick={() => !config.inspectorEnabled && onChange({ route: "home" })}>
                <FontAwesomeIcon icon={faHouse} /><span>首页</span>
              </button>
              <button type="button" className={config.route === "orders" ? "is-active" : ""} onClick={() => !config.inspectorEnabled && onChange({ route: "orders" })}>
                <FontAwesomeIcon icon={faReceipt} /><span>订单</span>
              </button>
              <button type="button"><FontAwesomeIcon icon={faUser} /><span>我的</span></button>
            </nav>

            {config.inspectorEnabled && (
              <div className="studio-inspector-hint">
                <FontAwesomeIcon icon={faCrosshairs} />
                {selectedElement ? `已选择：${selectedElement}` : "点击页面元素进行检查"}
              </div>
            )}
          </div>
        </div>
      </div>

      <footer className="studio-miniapp-footer">
        <span>{device.name} · {device.platform}</span>
        <span>{refreshing ? "正在刷新预览…" : config.inspectorEnabled ? "元素选择已开启" : "前端演示数据"}</span>
      </footer>
    </div>
  );
}

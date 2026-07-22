import { useEffect, useRef, useState } from "react";
import type { EChartsType } from "echarts/core";

interface EChartsBlockProps {
  content: string;
}

type EChartsCore = typeof import("echarts/core");

type EChartsInstallModule = {
  install: unknown;
};

const baseChartLoaders: Array<() => Promise<EChartsInstallModule>> = [
  () => import("echarts/lib/chart/bar/install.js"),
  () => import("echarts/lib/chart/line/install.js"),
  () => import("echarts/lib/chart/pie/install.js"),
  () => import("echarts/lib/chart/scatter/install.js"),
  () => import("echarts/lib/component/dataZoom/install.js"),
  () => import("echarts/lib/component/dataset/install.js"),
  () => import("echarts/lib/component/grid/install.js"),
  () => import("echarts/lib/component/legend/install.js"),
  () => import("echarts/lib/component/title/install.js"),
  () => import("echarts/lib/component/toolbox/install.js"),
  () => import("echarts/lib/component/tooltip/install.js"),
  () => import("echarts/lib/component/transform/install.js"),
  () => import("echarts/lib/component/visualMap/install.js"),
  () => import("echarts/lib/renderer/installCanvasRenderer.js"),
];

const advancedChartLoaders: Record<string, () => Promise<EChartsInstallModule[]>> = {
  gauge: async () => [await import("echarts/lib/chart/gauge/install.js")],
  graph: async () => [await import("echarts/lib/chart/graph/install.js")],
  heatmap: async () => [await import("echarts/lib/chart/heatmap/install.js")],
  radar: async () => [
    await import("echarts/lib/chart/radar/install.js"),
    await import("echarts/lib/component/radar/install.js"),
  ],
};

const registeredAdvancedCharts = new Set<string>();
let echartsCorePromise: Promise<EChartsCore> | null = null;
let baseChartsPromise: Promise<void> | null = null;

async function loadEChartsRuntime() {
  const core = await loadEChartsCore();
  await ensureBaseChartModules(core);
  return core;
}

function loadEChartsCore() {
  echartsCorePromise ??= import("echarts/core");
  return echartsCorePromise;
}

function ensureBaseChartModules(core: EChartsCore) {
  baseChartsPromise ??= Promise.all(baseChartLoaders.map((loader) => loader())).then((modules) => {
    core.use(modules.map((module) => module.install as any));
  });
  return baseChartsPromise;
}

function collectSeriesTypes(value: unknown, types = new Set<string>()): Set<string> {
  if (!value || typeof value !== "object") return types;
  if (Array.isArray(value)) {
    value.forEach((item) => collectSeriesTypes(item, types));
    return types;
  }

  const record = value as Record<string, unknown>;
  if (typeof record.type === "string") {
    types.add(record.type);
  }

  collectSeriesTypes(record.series, types);
  return types;
}

async function ensureAdvancedChartModules(core: EChartsCore, options: unknown) {
  const missingTypes = [...collectSeriesTypes((options as any)?.series)]
    .filter((type) => advancedChartLoaders[type])
    .filter((type) => !registeredAdvancedCharts.has(type));
  if (missingTypes.length === 0) return;

  const modules = (await Promise.all(missingTypes.map((type) => advancedChartLoaders[type]()))).flat();
  core.use(modules.map((module) => module.install as any));
  missingTypes.forEach((type) => registeredAdvancedCharts.add(type));
}

export function EChartsBlock({ content }: EChartsBlockProps) {
  const chartRef = useRef<HTMLDivElement>(null);
  const chartInstance = useRef<EChartsType | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [parsedOptions, setParsedOptions] = useState<any>(null);

  useEffect(() => {
    try {
      // Try to parse the content as JSON. If the model output a raw object, we try to safely evaluate or parse it.
      // Often models might output valid JSON, but sometimes they might put `const option = {...}`. 
      // Based on the prompt, it should be pure JSON.
      const rawText = content.trim();
      const options = JSON.parse(rawText);
      setParsedOptions(options);
      setError(null);
    } catch (err: any) {
      setError(`Failed to parse chart config: ${err.message}`);
    }
  }, [content]);

  useEffect(() => {
    if (!parsedOptions || !chartRef.current) return;

    let cancelled = false;
    let removeResizeHandler: (() => void) | null = null;

    const renderChart = async () => {
      try {
        const echarts = await loadEChartsRuntime();
        await ensureAdvancedChartModules(echarts, parsedOptions);
        if (cancelled || !chartRef.current) return;

        if (!chartInstance.current) {
          chartInstance.current = echarts.init(chartRef.current);
        }

        chartInstance.current.setOption(parsedOptions, true);
        const resizeHandler = () => {
          chartInstance.current?.resize();
        };
        window.addEventListener("resize", resizeHandler);
        removeResizeHandler = () => window.removeEventListener("resize", resizeHandler);
      } catch (err: any) {
        if (!cancelled) {
          setError(`ECharts error: ${err.message}`);
        }
      }
    };

    renderChart();

    return () => {
      cancelled = true;
      removeResizeHandler?.();
    };
  }, [parsedOptions]);

  // Cleanup chart instance on unmount
  useEffect(() => {
    return () => {
      if (chartInstance.current) {
        chartInstance.current.dispose();
        chartInstance.current = null;
      }
    };
  }, []);

  if (error) {
    return (
      <div className="mb-2 overflow-x-auto rounded-lg bg-red-950 px-3 py-2 text-[12px] leading-relaxed text-red-100 last:mb-0 border border-red-800">
        <div className="font-semibold mb-1">ECharts Error</div>
        <div>{error}</div>
        <pre className="mt-2 text-[10px] text-gray-400"><code>{content}</code></pre>
      </div>
    );
  }

  return (
    <div className="my-3 w-full rounded-lg bg-white p-2 shadow-sm dark:bg-gray-800 border border-gray-200 dark:border-gray-700">
      <div ref={chartRef} style={{ width: "100%", height: "350px" }} />
    </div>
  );
}

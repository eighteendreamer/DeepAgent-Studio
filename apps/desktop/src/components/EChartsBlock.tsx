import { useEffect, useRef, useState } from "react";
import * as echarts from "echarts";

interface EChartsBlockProps {
  content: string;
}

export function EChartsBlock({ content }: EChartsBlockProps) {
  const chartRef = useRef<HTMLDivElement>(null);
  const chartInstance = useRef<echarts.EChartsType | null>(null);
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

    if (!chartInstance.current) {
      chartInstance.current = echarts.init(chartRef.current);
    }

    try {
      chartInstance.current.setOption(parsedOptions, true);
    } catch (err: any) {
      setError(`ECharts error: ${err.message}`);
    }

    const resizeHandler = () => {
      chartInstance.current?.resize();
    };

    window.addEventListener("resize", resizeHandler);

    return () => {
      window.removeEventListener("resize", resizeHandler);
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

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

test("usage runtime renders and updates stacked line data with real ECharts modules", async () => {
  const dependencies = new Map(await Promise.all(
    ["echarts/core", "echarts/charts", "echarts/components", "echarts/renderers"]
      .map(async name => [name, await import(name)]),
  ));
  const source = fs.readFileSync(path.join(__dirname,
    "../src/components/settings/usageChartRuntime.ts"), "utf8");
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  });
  const runtime = {};
  new Function("exports", "require", outputText)(runtime, name => {
    assert.ok(dependencies.has(name), `Unexpected runtime dependency: ${name}`);
    return dependencies.get(name);
  });
  const core = dependencies.get("echarts/core");
  core.use(dependencies.get("echarts/renderers").SVGRenderer);
  const chart = runtime.init(null, null, { renderer: "svg", ssr: true, width: 640, height: 320 });
  try {
    const data = [[100, 0, 300], [80, 0, 120], [20, 0, 50]];
    chart.setOption({ animation: false, tooltip: { trigger: "axis" },
      xAxis: { type: "category", data: ["11/17", "11/18", "11/19"] },
      yAxis: { type: "value" },
      series: data.map((values, index) => ({ id: String(index), type: "line",
        stack: "tokens", areaStyle: {}, data: values })),
    });
    assert.deepEqual(chart.getOption().series.map(series => series.data), data);
    assert.match(chart.renderToSVGString(), /<path\b/);
    const updated = [[0, 50], [0, 30], [0, 20]];
    chart.setOption({ xAxis: { data: ["10月", "11月"] },
      series: updated.map((values, index) => ({ id: String(index), data: values })),
    });
    assert.deepEqual(chart.getOption().series.map(series => series.data), updated);
    assert.match(chart.renderToSVGString(), /11月/);
  } finally {
    chart.dispose();
  }
});

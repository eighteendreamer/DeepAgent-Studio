import { init, use } from "echarts/core";
import { LineChart } from "echarts/charts";
import { GridComponent, TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";

// Keep the public entry points in one lazy module with named imports so the
// runtime stays consistent and production builds can remove unused chart types.
use([LineChart, GridComponent, TooltipComponent, CanvasRenderer]);

export { init };

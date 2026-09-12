import type { LucideIcon } from "lucide-react";
import {
  User, Mountain, Package, Palette, Image, Wrench,
  FileText, Presentation, Trees,
  Sparkles, LayoutGrid, Home, Paintbrush,
  Ruler, BarChart3, LayoutDashboard, Lightbulb,
  Sun, Brush, RotateCcw, PenTool,
  Building2, TreePine, Map, Sofa, Hammer,
  Folder, Box,
} from "lucide-react";

export interface CategoryNode {
  key: string;
  label: string;
  icon: LucideIcon;
  children?: CategoryNode[];
}

export const CATEGORY_TREE: CategoryNode[] = [
  { key: "character", label: "人物", icon: User },
  { key: "scene", label: "场景", icon: Mountain },
  { key: "product", label: "产品", icon: Package },
  { key: "art", label: "艺术", icon: Palette },
  { key: "poster", label: "海报", icon: Image },
  {
    key: "tool", label: "工具", icon: Wrench,
    children: [
      { key: "wechat_cover", label: "微信公众号封面模版", icon: FileText },
      { key: "ppt_template", label: "PPT模板制作", icon: Presentation },
    ],
  },
  {
    key: "environment_design", label: "环境设计", icon: Trees,
    children: [
      {
        key: "effect_rendering", label: "效果渲染", icon: Sparkles,
        children: [
          { key: "arch_render", label: "建筑渲染", icon: Building2 },
          { key: "interior_render", label: "室内渲染", icon: Home },
          { key: "landscape_render", label: "景观渲染", icon: TreePine },
          { key: "planning_render", label: "规划渲染", icon: Map },
        ],
      },
      {
        key: "masterplan_coloring", label: "总平填色", icon: LayoutGrid,
        children: [
          { key: "arch_masterplan", label: "建筑总平填色", icon: Building2 },
          { key: "landscape_masterplan", label: "景观总平填色", icon: TreePine },
          { key: "planning_masterplan", label: "规划总平填色", icon: Map },
        ],
      },
      {
        key: "floorplan_coloring", label: "户型填色", icon: Home,
        children: [
          { key: "home_floorplan", label: "家装平面", icon: Home },
          { key: "commercial_floorplan", label: "公装平面", icon: Building2 },
        ],
      },
      {
        key: "style_transfer", label: "风格迁移", icon: Paintbrush,
        children: [
          { key: "facade_transfer", label: "建筑立面迁移", icon: Building2 },
          { key: "atmosphere_transfer", label: "建筑氛围迁移", icon: Sun },
          { key: "interior_element_transfer", label: "室内元素迁移", icon: Sofa },
          { key: "plan_style_transfer", label: "平面风格迁移", icon: LayoutGrid },
        ],
      },
      {
        key: "section_elevation", label: "剖立面", icon: Ruler,
        children: [
          { key: "elevation", label: "立面", icon: Building2 },
          { key: "section", label: "剖面", icon: Ruler },
          { key: "section_perspective", label: "剖透视", icon: PenTool },
        ],
      },
      {
        key: "analysis_diagram", label: "分析图", icon: BarChart3,
        children: [
          { key: "site_analysis", label: "场地分析图", icon: Map },
          { key: "building_analysis", label: "建筑分析图", icon: Building2 },
          { key: "interior_analysis", label: "室内分析图", icon: Home },
          { key: "landscape_analysis", label: "景观分析图", icon: TreePine },
          { key: "planning_analysis", label: "规划分析图", icon: Map },
        ],
      },
      {
        key: "board_design", label: "展板设计", icon: LayoutDashboard,
        children: [
          { key: "arch_board", label: "建筑展板生成", icon: Building2 },
          { key: "interior_board", label: "室内展板生成", icon: Home },
          { key: "landscape_board", label: "景观展板生成", icon: TreePine },
          { key: "planning_board", label: "规划展板生成", icon: Map },
        ],
      },
      {
        key: "inspiration_generation", label: "灵感生成", icon: Lightbulb,
        children: [
          { key: "arch_inspiration", label: "建筑设计灵感", icon: Building2 },
          { key: "facade_inspiration", label: "立面设计灵感", icon: Building2 },
          { key: "home_inspiration", label: "家居设计灵感", icon: Home },
          { key: "interior_inspiration", label: "室内设计灵感", icon: Sofa },
          { key: "landscape_inspiration", label: "景观设计灵感", icon: TreePine },
        ],
      },
      {
        key: "ambience_transformation", label: "氛围转换", icon: Sun,
        children: [
          { key: "light_shadow", label: "光影转换", icon: Sun },
          { key: "color_tone", label: "色调转换", icon: Palette },
          { key: "time_transform", label: "时间转换", icon: Sun },
          { key: "season_transform", label: "四季转换", icon: TreePine },
          { key: "weather_transform", label: "天气转换", icon: Sun },
        ],
      },
      {
        key: "art_style_conversion", label: "画风转换", icon: Brush,
        children: [
          { key: "illustration_style", label: "插画风格", icon: Brush },
          { key: "collage_style", label: "拼贴风格", icon: LayoutGrid },
          { key: "line_art_style", label: "线稿风格", icon: PenTool },
          { key: "white_model_style", label: "白模风格", icon: Box },
          { key: "model_style", label: "模型风格", icon: Box },
          { key: "competition_style", label: "竞赛风格", icon: Folder },
        ],
      },
      {
        key: "view_transform", label: "视角转换", icon: RotateCcw,
        children: [
          { key: "plan_aerial", label: "平面鸟瞰互转", icon: Map },
          { key: "perspective_view", label: "透视视角", icon: PenTool },
          { key: "home_plan_3d", label: "家装平面转三维", icon: Home },
          { key: "three_view", label: "三视图", icon: Ruler },
        ],
      },
      {
        key: "scheme_design", label: "方案设计", icon: PenTool,
        children: [
          { key: "arch_scheme", label: "建筑方案", icon: Building2 },
          { key: "landscape_scheme", label: "景观方案", icon: TreePine },
          { key: "planning_scheme", label: "规划方案", icon: Map },
          { key: "interior_scheme", label: "室内方案", icon: Sofa },
        ],
      },
      { key: "old_house_renovation", label: "旧房改造", icon: Hammer },
      { key: "interior_decoration", label: "室内装修", icon: Sofa },
      { key: "local_modification", label: "局部修改", icon: PenTool },
    ],
  },
  { key: "other", label: "其他", icon: Package },
];

// Flatten tree to get all leaf keys
export function getAllCategoryKeys(nodes: CategoryNode[]): string[] {
  const keys: string[] = [];
  for (const node of nodes) {
    if (node.children && node.children.length > 0) {
      keys.push(...getAllCategoryKeys(node.children));
    } else {
      keys.push(node.key);
    }
  }
  return keys;
}

// Find node by key
export function findCategoryNode(nodes: CategoryNode[], key: string): CategoryNode | null {
  for (const node of nodes) {
    if (node.key === key) return node;
    if (node.children) {
      const found = findCategoryNode(node.children, key);
      if (found) return found;
    }
  }
  return null;
}

// Get breadcrumb path for a category key
export function getCategoryPath(nodes: CategoryNode[], key: string, path: CategoryNode[] = []): CategoryNode[] | null {
  for (const node of nodes) {
    const currentPath = [...path, node];
    if (node.key === key) return currentPath;
    if (node.children) {
      const found = getCategoryPath(node.children, key, currentPath);
      if (found) return found;
    }
  }
  return null;
}

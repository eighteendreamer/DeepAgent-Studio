import React from "react";

interface IconProps extends React.SVGProps<SVGSVGElement> {}

export const SidebarLeftIcon = (props: IconProps) => (
  <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" {...props}>
    <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" />
    <path d="M5.5 2.5V13.5" />
  </svg>
);

export const BottomPanelIcon = (props: IconProps) => (
  <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" {...props}>
    <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" />
    <path d="M1.5 9.5H14.5" />
  </svg>
);

export const SidebarRightIcon = (props: IconProps) => (
  <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" {...props}>
    <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" />
    <path d="M10.5 2.5V13.5" />
  </svg>
);

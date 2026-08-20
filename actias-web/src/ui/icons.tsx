/**
 * Line icons, paths verbatim from the design output's sidebar: single
 * stroke weight, no fill, the same grammar as the moth mark.
 */
import React from 'react';

const paths: Record<string, string[]> = {
  projects: [
    'M4 4h6v8h-6z',
    'M4 16h6v4h-6z',
    'M14 12h6v8h-6z',
    'M14 4h6v4h-6z',
  ],
  overview: ['M3 12h4l3 8l4 -16l3 8h4'],
  scripts: ['M7 8l-4 4l4 4', 'M17 8l4 4l-4 4', 'M14 4l-4 16'],
  kv: [
    'M12 3l8 4.5v9l-8 4.5l-8 -4.5v-9z',
    'M12 12l8 -4.5',
    'M12 12v9',
    'M12 12l-8 -4.5',
  ],
  databases: [
    'M12 6m-8 0a8 3 0 1 0 16 0a8 3 0 1 0 -16 0',
    'M4 6v6a8 3 0 0 0 16 0v-6',
    'M4 12v6a8 3 0 0 0 16 0v-6',
  ],
  queues: [
    'M4 13h3l3 3h4l3 -3h3',
    'M5 19h14a2 2 0 0 0 2 -2v-10a2 2 0 0 0 -2 -2h-14a2 2 0 0 0 -2 2v10a2 2 0 0 0 2 2z',
  ],
  members: [
    'M9 7m-4 0a4 4 0 1 0 8 0a4 4 0 1 0 -8 0',
    'M3 21v-2a4 4 0 0 1 4 -4h4a4 4 0 0 1 4 4v2',
    'M16 3.13a4 4 0 0 1 0 7.75',
    'M21 21v-2a4 4 0 0 0 -3 -3.85',
  ],
  settings: [
    'M10.325 4.317c.426 -1.756 2.924 -1.756 3.35 0a1.724 1.724 0 0 0 2.573 1.066c1.543 -.94 3.31 .826 2.37 2.37a1.724 1.724 0 0 0 1.065 2.572c1.756 .426 1.756 2.924 0 3.35a1.724 1.724 0 0 0 -1.066 2.573c.94 1.543 -.826 3.31 -2.37 2.37a1.724 1.724 0 0 0 -2.572 1.065c-.426 1.756 -2.924 1.756 -3.35 0a1.724 1.724 0 0 0 -2.573 -1.066c-1.543 .94 -3.31 -.826 -2.37 -2.37a1.724 1.724 0 0 0 -1.065 -2.572c-1.756 -.426 -1.756 -2.924 0 -3.35a1.724 1.724 0 0 0 1.066 -2.573c-.94 -1.543 .826 -3.31 2.37 -2.37c1 .608 2.296 .07 2.572 -1.065z',
    'M12 12m-3 0a3 3 0 1 0 6 0a3 3 0 1 0 -6 0',
  ],
  download: [
    'M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2 -2v-2',
    'M7 11l5 5l5 -5',
    'M12 4l0 12',
  ],
};

export type IconName = keyof typeof paths;

export function Icon({ name, size = 15 }: { name: IconName; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      {paths[name].map((d) => (
        <path key={d} d={d} />
      ))}
    </svg>
  );
}

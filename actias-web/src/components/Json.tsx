import React from 'react';
import JsonView from '@uiw/react-json-view';
import { darkTheme } from '@uiw/react-json-view/dark';

/** Structured values in the console's own dark theme; the viewer's own
 * background yields to the surface it sits on. */
export const Json: React.FC<{ value: object }> = ({ value }) => (
  <JsonView
    value={value}
    style={{
      ...(darkTheme as React.CSSProperties),
      background: 'transparent',
      fontFamily: 'var(--mono)',
      fontSize: 12,
    }}
  />
);

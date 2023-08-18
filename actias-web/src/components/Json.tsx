import { lightTheme } from '@uiw/react-json-view/light';
import { darkTheme } from '@uiw/react-json-view/dark';
import React from 'react';
import JsonView from '@uiw/react-json-view';
import { useMantineColorScheme } from '@mantine/core';

export const Json: React.FC<{ value: any }> = ({ value }) => {
  const { colorScheme } = useMantineColorScheme();

  return (
    <JsonView
      value={value}
      style={{
        ...((colorScheme === 'dark' ? darkTheme : lightTheme) as any),
        background: 'transparent',
      }}
    />
  );
};

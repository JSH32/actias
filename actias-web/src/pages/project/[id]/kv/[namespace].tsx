import { PairDto } from '@/client';
import { useRouter } from 'next/router';
import React, { useCallback, useEffect, useState } from 'react';
import api, { showError } from '@/helpers/api';
import { withAuthentication } from '@/helpers/authenticated';
import { Json } from '@/components/Json';
import { ActionIcon, Button, Loader, Select, Table } from '@mantine/core';
import { IconTrash } from '@tabler/icons-react';
import { it } from 'node:test';

const Namespace = () => {
  const router = useRouter();

  // const [namespace, setNamespace] = useState<NamespaceDto | null>(null);
  const [token, setToken] = useState<string | undefined>(undefined);
  const [items, setItems] = useState<PairDto[]>([]);

  const loadNextPage = useCallback(() => {
    api.kv
      .listNamespace(
        router.query.id as string,
        router.query.namespace as string,
        token,
      )
      .then((res) => {
        setItems([...items, ...res.pairs]);
        setToken(res.token);
      })
      .catch(showError);
  }, [items, token, router]);

  useEffect(() => {
    loadNextPage();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return items.length ? (
    <>
      <Table>
        <thead>
          <tr>
            <th>Key</th>
            <th>Type</th>
            <th>Value</th>
            <th>Actions</th>
          </tr>
        </thead>
        <tbody>
          {items.map((item) => (
            <Pair key={item.key} pair={item} />
          ))}
        </tbody>
      </Table>
      <Button onClick={loadNextPage}>Load More</Button>
    </>
  ) : (
    <Loader />
  );
};

const Pair: React.FC<{ pair: PairDto }> = ({ pair }) => {
  const [currentType, setCurrentType] = useState<string>(
    pair.type as unknown as string,
  );

  const [currentValue, setCurrentValue] = useState<any>(pair.value);

  return (
    <tr>
      <td>{pair.key}</td>
      <td>
        <Select
          data={['JSON', 'NUMBER', 'INTEGER', 'BOOLEAN', 'STRING'].map(
            (item) => ({ value: item, label: item }),
          )}
          onChange={(val) => setCurrentType(val as any)}
          value={currentType}
        />
      </td>
      <td>{currentValue}</td>
      <td>
        <ActionIcon variant="default" onClick={() => {}} size={30}>
          <IconTrash size="1rem" />
        </ActionIcon>
      </td>
    </tr>
  );
};

export default withAuthentication(Namespace);

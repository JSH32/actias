import { AclListDto, PairDto, ProjectDto, SetKeyDto } from '@/client';
import { useRouter } from 'next/router';
import React, { useCallback, useEffect, useState } from 'react';
import api, { showError } from '@/helpers/api';
import { withAuthentication } from '@/helpers/authenticated';
import {
  ActionIcon,
  Badge,
  Box,
  Breadcrumbs,
  Button,
  Code,
  Group,
  Loader,
  Modal,
  Select,
  Table,
  Text,
  TextInput,
} from '@mantine/core';
import {
  IconCopy,
  IconEdit,
  IconFilePlus,
  IconTrash,
} from '@tabler/icons-react';
import { notifications } from '@mantine/notifications';
import { breadcrumbs } from '@/helpers/util';
import { useClipboard, useDisclosure } from '@mantine/hooks';
import { JsonInput } from '@mantine/core';
import { Json } from '@/components/Json';

const Namespace = () => {
  const router = useRouter();

  const [token, setToken] = useState<string | undefined>(undefined);
  const [items, setItems] = useState<PairDto[]>([]);
  const [parentProject, setParentProject] = useState<ProjectDto | null>(null);

  const [acl, setAcl] = useState<AclListDto | null>();

  const loadNextPage = useCallback(
    (reload: boolean = false) => {
      if (reload) {
        setItems([]);
        setToken(undefined);
      }

      api.kv
        .listNamespace(
          router.query.id as string,
          router.query.namespace as string,
          token,
        )
        .then((res) => {
          if (res.pairs.length) {
            setItems(res.pairs);
            setToken(res.token);
          } else if (
            res.pairs.length < 1 &&
            (reload || (items?.length || 0) < 1)
          ) {
            notifications.show({
              title: 'Namespace is empty!',
              color: 'red',
              message: `${router.query.namespace} is empty. Nothing to see here.`,
            });

            router.push(`/project/${router.query.id}`);
          }
        })
        .catch(showError);
    },
    [items, token, router],
  );

  const reloadKey = useCallback(
    (key: string) => {
      api.kv
        .getKey(
          router.query.id as string,
          router.query.namespace as string,
          key,
        )
        .then((pair) => {
          // Replace the item in place when reloaded.
          setItems(items.map((item) => (item.key === pair.key ? pair : item)));
        });
    },
    [items, router],
  );

  const [addItemOpened, addItemControl] = useDisclosure(false);
  const addPair = useCallback(
    (key: string, newPair: SetKeyDto) => {
      api.kv
        .setKey(
          router.query.id as string,
          router.query.namespace as string,
          key,
          {
            type: newPair.type,
            value: newPair.value,
          },
        )
        .then(() => {
          notifications.show({
            title: 'Key set!',
            message: `Key ${key} was set.`,
          });

          loadNextPage(true);
        })
        .catch(showError);
    },
    [router, loadNextPage],
  );

  useEffect(() => {
    api.project
      .getProject(router.query.id as string)
      .then(setParentProject)
      .catch(showError);

    api.acl
      .getAclMe(router.query.id as string)
      .then(setAcl)
      .catch(showError);

    loadNextPage();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return items.length ? (
    <>
      <Breadcrumbs>
        {breadcrumbs([
          { title: 'Home', href: '/projects' },
          {
            title: parentProject?.name as string,
            href: `/project/${parentProject?.id}`,
          },
          { title: 'kv', href: `/project/${parentProject?.id}` },
          {
            title: router.query.namespace as string,
            href: `/kv/${router.query.namespace}`,
          },
        ])}
      </Breadcrumbs>

      <Table>
        <Table.Thead>
          <Table.Tr>
            <Table.Th>Key</Table.Th>
            <Table.Th>Type</Table.Th>
            <Table.Th>Value</Table.Th>
            {acl?.permissions['KV_WRITE'] && <Table.Th>Actions</Table.Th>}
          </Table.Tr>
        </Table.Thead>
        <Table.Tbody>
          {items.map((item) => (
            <Pair
              write={acl?.permissions['KV_WRITE']}
              reload={() => reloadKey(item.key)}
              // This is used in case the value changes.
              key={`${item.key}-${item.type}-${item.value}`}
              pair={item}
              onDelete={() => loadNextPage(true)}
            />
          ))}
        </Table.Tbody>
      </Table>
      <EditModal
        onSubmit={addPair}
        onClose={addItemControl.close}
        opened={addItemOpened}
      />
      {acl?.permissions['KV_WRITE'] && (
        <Button onClick={() => addItemControl.open()}>Create Item</Button>
      )}

      {token && <Button onClick={() => loadNextPage()}>Load More</Button>}
    </>
  ) : (
    <Loader />
  );
};

const Pair: React.FC<{
  pair: PairDto;
  write?: boolean;
  onDelete: () => void;
  reload: () => void;
}> = ({ pair, onDelete, reload, write }) => {
  const deletePair = useCallback(() => {
    api.kv
      .deleteKey(pair.projectId, pair.namespace, pair.key)
      .then(() => {
        notifications.show({
          title: 'Key deleted!',
          message: `Key ${pair.key} was deleted.`,
        });

        onDelete();
      })
      .catch(showError);
  }, [onDelete, pair]);

  const editPair = useCallback(
    (_: string, newPair: SetKeyDto) => {
      api.kv
        .setKey(pair.projectId, pair.namespace, pair.key, {
          type: newPair.type,
          value: newPair.value,
        })
        .then(() => {
          notifications.show({
            title: 'Key edited!',
            message: `Key ${pair.key} was edited.`,
          });
          reload();
        })
        .catch(showError);
    },
    [pair, reload],
  );

  const [opened, { open, close }] = useDisclosure(false);
  const clipboard = useClipboard();

  return (
    <>
      <EditModal
        pair={pair}
        onSubmit={editPair}
        onClose={close}
        opened={opened}
      />

      <Table.Tr>
        <Table.Td>{pair.key}</Table.Td>
        <Table.Td>
          <Badge>{pair.type}</Badge>
        </Table.Td>
        <Table.Td>
          {(pair.type as unknown as string) === 'JSON' ? (
            <Box miw={'200px'}>
              <Json value={JSON.parse(pair.value)} />
            </Box>
          ) : (
            <Text>{pair.value}</Text>
          )}
        </Table.Td>
        {write && (
          <Table.Td>
            <Group>
              <ActionIcon
                variant="default"
                onClick={() => {
                  clipboard.copy(pair.value);
                  notifications.show({
                    title: 'Copied',
                    message: `Copied ${pair.key} to clipboard.`,
                  });
                }}
                size={30}
              >
                <IconCopy size="1rem" />
              </ActionIcon>
              <ActionIcon variant="default" onClick={deletePair} size={30}>
                <IconTrash size="1rem" />
              </ActionIcon>
              <ActionIcon variant="default" onClick={open} size={30}>
                <IconEdit size="1rem" />
              </ActionIcon>
            </Group>
          </Table.Td>
        )}
      </Table.Tr>
    </>
  );
};

const EditModal: React.FC<{
  pair?: PairDto;
  onSubmit: (key: string, value: SetKeyDto) => void;
  onClose: () => void;
  opened: boolean;
}> = ({ pair, onSubmit, onClose, opened }) => {
  const [currentType, setCurrentType] = useState<string>(
    (pair?.type as unknown as string) || 'STRING',
  );

  const [currentValue, setCurrentValue] = useState<any>(pair?.value || 'Hello');
  const [key, setKey] = useState<string>(pair?.key || '');

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title={
        pair ? (
          <Group>
            <IconEdit size={20} />
            Edit key <Code>{pair.key}</Code>
          </Group>
        ) : (
          <Group>
            <IconFilePlus size={20} />
            Create key
          </Group>
        )
      }
    >
      {!pair && (
        <TextInput
          placeholder="Key"
          label="Pair key"
          withAsterisk
          value={key}
          onChange={(event) => setKey(event.currentTarget.value)}
        />
      )}

      <Select
        data={['JSON', 'NUMBER', 'INTEGER', 'BOOLEAN', 'STRING'].map(
          (item) => ({ value: item, label: item }),
        )}
        onChange={(val) => setCurrentType(val as any)}
        label="Value type"
        value={currentType}
      />
      {currentType === 'JSON' ? (
        <JsonInput
          placeholder="Value"
          label="Pair value"
          validationError="Invalid JSON"
          formatOnBlur
          autosize
          withAsterisk
          value={currentValue}
          onChange={(value) => setCurrentValue(value)}
        />
      ) : (
        <TextInput
          placeholder="Value"
          label="Pair value"
          withAsterisk
          value={currentValue}
          onChange={(event) => setCurrentValue(event.currentTarget.value)}
        />
      )}

      <Group align="right" mt="md">
        <Button
          type="submit"
          onClick={() => {
            onSubmit(key, { type: currentType, value: currentValue });
            onClose();
          }}
        >
          {pair ? 'Edit' : 'Create'} pair
        </Button>
      </Group>
    </Modal>
  );
};

export default withAuthentication(Namespace);

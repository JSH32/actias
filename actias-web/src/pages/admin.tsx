import { PaginatedResponseDto, RegistrationCodeDto } from '@/client';
import { AuthGuard } from '@/helpers/auth';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  ActionIcon,
  Button,
  Divider,
  Group,
  Modal,
  NumberInput,
  Pagination,
  Stack,
  Table,
  Title,
} from '@mantine/core';
import { useCallback, useState } from 'react';
import api, { showError } from '@/helpers/api';
import { IconCopy, IconTrash } from '@tabler/icons-react';
import { notifications } from '@mantine/notifications';
import { useClipboard, useDisclosure } from '@mantine/hooks';

const Admin = () => {
  const [page, setPage] = useState(1);
  const queryClient = useQueryClient();

  const { data: codes } = useQuery<PaginatedResponseDto>({
    queryKey: ['registration-codes', page],
    queryFn: () => api.admin.listRegistrationCodes(page),
  });

  const reloadCodes = useCallback(() => {
    setPage(1);
    queryClient.invalidateQueries({ queryKey: ['registration-codes'] });
  }, [queryClient]);

  const deleteCode = useCallback(
    (code: string) => {
      api.admin
        .deleteRegistrationCode(code)
        .then(() => {
          notifications.show({
            title: 'Deleted code!',
            message: `Deleted code: ${code}`,
          });

          reloadCodes();
        })
        .catch(showError);
    },
    [reloadCodes],
  );

  const [codeModalOpened, codeModalControl] = useDisclosure(false);

  const createCode = useCallback(
    (uses: number) => {
      api.admin
        .newRegistrationCode(uses)
        .then(() => {
          notifications.show({
            title: 'Created code!',
            message: `Created code with ${uses} uses`,
          });

          codeModalControl.close();
          reloadCodes();
        })
        .catch(showError);
    },
    [codeModalControl, reloadCodes],
  );

  const clipboard = useClipboard();

  return (
    <Stack>
      <Title>Admin</Title>
      <Divider />
      <Stack>
        <Title order={2}>Registration Codes</Title>
        <Table>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Code</Table.Th>
              <Table.Th>Uses</Table.Th>
              <Table.Th>Created At</Table.Th>
              <Table.Th>Actions</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {(codes as any)?.items?.map((code: RegistrationCodeDto) => (
              <Table.Tr key={code.id}>
                <Table.Td>{code.id}</Table.Td>
                <Table.Td>{code.uses}</Table.Td>
                <Table.Td>{code.createdAt}</Table.Td>
                <Table.Td>
                  <Group>
                    <ActionIcon
                      variant="default"
                      onClick={() => deleteCode(code.id)}
                      size={30}
                    >
                      <IconTrash size="1rem" />
                    </ActionIcon>
                    <ActionIcon
                      variant="default"
                      onClick={() => {
                        clipboard.copy(code.id);
                        notifications.show({
                          title: 'Copied',
                          message: `Copied ${code.id} to clipboard.`,
                        });
                      }}
                      size={30}
                    >
                      <IconCopy size="1rem" />
                    </ActionIcon>
                  </Group>
                </Table.Td>
              </Table.Tr>
            ))}
          </Table.Tbody>
        </Table>
        <Pagination
          value={codes?.page}
          onChange={setPage}
          total={codes?.lastPage || 1}
        />
        <CreateRegistrationModal
          onSubmit={createCode}
          onClose={codeModalControl.close}
          opened={codeModalOpened}
        />
        <Button w={'120px'} onClick={() => codeModalControl.open()}>
          Create Code
        </Button>
      </Stack>
    </Stack>
  );
};

const CreateRegistrationModal: React.FC<{
  onSubmit: (uses: number) => void;
  onClose: () => void;
  opened: boolean;
}> = ({ onSubmit, onClose, opened }) => {
  const [uses, setUses] = useState<number>(1);

  return (
    <Modal opened={opened} onClose={onClose} title="Create registration code">
      <NumberInput
        placeholder="Uses"
        label="Code uses"
        withAsterisk
        value={uses}
        onChange={(value) => setUses(Number(value))}
      />

      <Group align="right" mt="md">
        <Button
          type="submit"
          onClick={() => {
            onSubmit(uses);
            onClose();
          }}
        >
          Create Code
        </Button>
      </Group>
    </Modal>
  );
};

export default function AdminPage() {
  return (
    <AuthGuard>
      <Admin />
    </AuthGuard>
  );
}

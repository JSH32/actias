import { AclListDto, ProjectDto, UserDto } from '@/client';
import {
  Text,
  Group,
  Modal,
  Select,
  Stack,
  Title,
  MultiSelect,
  ScrollArea,
  Button,
  Grid,
  Card,
  Badge,
  CloseButton,
  Anchor,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { forwardRef, useCallback, useEffect, useState } from 'react';
import api, { showError } from '@/helpers/api';
import { IconUser, IconUserPlus } from '@tabler/icons-react';
import { notifications } from '@mantine/notifications';

const AccessControl: React.FC<{ project: ProjectDto; write: boolean }> = ({
  project,
  write,
}) => {
  const [addUserModalOpened, addUserModal] = useDisclosure(false);

  const [users, setUsers] = useState<UserDto[]>([]);
  const [currentUser, setCurrentUser] = useState<UserDto | null>(null);
  const [allPermissions, setAllPermissions] = useState<string[]>([]);
  const [permissions, setPermissions] = useState<string[]>([]);

  const [accessUsers, setAccessUsers] = useState<AclListDto[]>([]);

  const loadAcl = useCallback(() => {
    api.acl.getAcl(project.id).then(setAccessUsers).catch(showError);
  }, [project]);

  useEffect(() => {
    api.acl.getPermissions().then(setAllPermissions).catch(showError);
    loadAcl();
  }, [project, loadAcl]);

  const searchUsers = useCallback((query: string) => {
    api.users
      .searchUsers(query, 1)
      .then((res) => setUsers((res as any).items))
      .catch(showError);
  }, []);

  const createAccess = useCallback(() => {
    api.acl
      .putAcl(currentUser!.id, project.id, permissions)
      .then(() => {
        notifications.show({
          title: 'Created access',
          message: `${currentUser!.username} has been granted access.`,
        });

        addUserModal.close();
        loadAcl();
      })
      .catch(showError);
  }, [addUserModal, currentUser, loadAcl, permissions, project]);

  const deleteAcl = useCallback(
    (user: UserDto) => {
      api.acl
        .putAcl(user.id, project.id, [])
        .then(() => loadAcl())
        .catch(showError);
    },
    [project, loadAcl],
  );

  const userModal = useCallback(
    (data: AclListDto) => {
      setCurrentUser(data.user);
      setPermissions(
        Object.keys(data.permissions).filter(
          (perm) => data.permissions[perm] !== false,
        ),
      );
      addUserModal.open();
    },
    [addUserModal],
  );

  return (
    <Stack>
      <Title>Access</Title>
      {write && (
        <Select
          maw="300px"
          label="Add user"
          placeholder="Search users"
          nothingFound="User not found"
          onSearchChange={searchUsers}
          onChange={(user) => {
            setCurrentUser(user as any);
            setPermissions([]);
            addUserModal.open();
          }}
          itemComponent={SelectItem}
          searchable
          data={users.map((user) => ({
            user: user,
            label: user.username,
            value: user as any,
          }))}
        />
      )}

      <Grid gutter="xs">
        {accessUsers.map((user) => (
          <Grid.Col key={user.user.id} md={6} lg={3}>
            <Card shadow="sm" padding="sm" radius="md" withBorder>
              <Group position="apart">
                <Group>
                  <IconUser />
                  <SelectItem user={user.user} />
                </Group>
                {write && (
                  <Anchor
                    component="button"
                    onClick={() => deleteAcl(user.user)}
                  >
                    <CloseButton aria-label="Delete project" />
                  </Anchor>
                )}
              </Group>
              <Stack>
                <Group spacing="xs">
                  {Object.entries(user.permissions).map(
                    ([perm, enabled]: any) =>
                      enabled && (
                        <Badge variant="light" key={perm}>
                          {perm}
                        </Badge>
                      ),
                  )}
                </Group>
                {write && <Button onClick={() => userModal(user)}>Edit</Button>}
              </Stack>
            </Card>
          </Grid.Col>
        ))}
      </Grid>

      <Modal
        opened={addUserModalOpened}
        onClose={addUserModal.close}
        title="Add User"
        scrollAreaComponent={ScrollArea.Autosize}
      >
        <Group>
          <IconUserPlus />
          {currentUser && <SelectItem user={currentUser!} />}
        </Group>
        <MultiSelect
          clearable
          withinPortal
          dropdownPosition="bottom"
          value={permissions}
          data={allPermissions.map((perm) => ({ value: perm, label: perm }))}
          label="Permissions for the user"
          onChange={setPermissions}
        ></MultiSelect>
        <Group position="right" mt="md">
          <Button type="submit" onClick={() => createAccess()}>
            Add User
          </Button>
        </Group>
      </Modal>
    </Stack>
  );
};

interface UserProps extends React.ComponentPropsWithoutRef<'div'> {
  user: UserDto;
}

// eslint-disable-next-line react/display-name
const SelectItem = forwardRef<HTMLDivElement, UserProps>(
  ({ user, ...others }, ref) => {
    return (
      <div ref={ref} {...others}>
        <Group noWrap>
          <div>
            <Text size="sm">{user.username}</Text>
            <Text size="xs" opacity={0.65}>
              {user.email}
            </Text>
          </div>
        </Group>
      </div>
    );
  },
);

export default AccessControl;

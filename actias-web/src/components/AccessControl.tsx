import { AclListDto, ProjectDto, UserDto } from '@/client';
import {
  Text,
  Group,
  Modal,
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
  TextInput,
  Combobox,
  useCombobox,
  Divider,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import React, { useCallback, useEffect, useState } from 'react';
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

  const searchUsers = useCallback((name: string) => {
    api.users
      .searchUsers(name, 1)
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
        .then(() => {
          notifications.show({
            title: 'Deleted access',
            message: `${currentUser!.username} access has been unset.`,
          });

          loadAcl();
        })
        .catch(showError);
    },
    [project.id, currentUser, loadAcl],
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

  useEffect(() => {
    api.acl.getPermissions().then(setAllPermissions).catch(showError);
    loadAcl();
    searchUsers('');
  }, [project, loadAcl, searchUsers]);

  const combobox = useCombobox({
    onDropdownClose: () => combobox.resetSelectedOption(),
  });

  return (
    <Stack>
      <Title>Access</Title>
      <Divider />
      {write && (
        <Combobox
          store={combobox}
          onOptionSubmit={(user) => {
            setCurrentUser(user as any);
            setPermissions([]);
            addUserModal.open();
            combobox.closeDropdown();
          }}
        >
          <Combobox.Target>
            <TextInput
              style={{
                width: '300px',
              }}
              label="Add user"
              placeholder="Search users"
              rightSection={<Combobox.Chevron />}
              onChange={(event) => {
                combobox.openDropdown();
                combobox.updateSelectedOptionIndex();
                searchUsers(event.currentTarget.value);
              }}
              onClick={() => combobox.openDropdown()}
              onFocus={() => combobox.openDropdown()}
              onBlur={() => combobox.closeDropdown()}
            />
          </Combobox.Target>

          <Combobox.Dropdown>
            <Combobox.Options>
              {users.map((item) => (
                <Combobox.Option value={item as any} key={item.id}>
                  <SelectItem user={item} />
                </Combobox.Option>
              ))}
            </Combobox.Options>
          </Combobox.Dropdown>
        </Combobox>
      )}

      <Grid gutter="xs">
        {accessUsers.map((user) => (
          <Grid.Col key={user.user.id} span={{ md: 6, lg: 3 }}>
            <Card shadow="sm" padding="sm" radius="md" withBorder>
              <Stack>
                <Group justify="space-between">
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
                  <Group justify="flex-start">
                    {Object.entries(user.permissions).map(
                      ([perm, enabled]: any) =>
                        enabled && (
                          <Badge variant="light" key={perm}>
                            {perm}
                          </Badge>
                        ),
                    )}
                  </Group>
                  {write && (
                    <Button onClick={() => userModal(user)}>Edit</Button>
                  )}
                </Stack>
              </Stack>
            </Card>
          </Grid.Col>
        ))}
      </Grid>

      <Modal
        opened={addUserModalOpened}
        onClose={addUserModal.close}
        title="User access"
        scrollAreaComponent={ScrollArea.Autosize}
      >
        <Group>
          <IconUserPlus />
          {currentUser && <SelectItem user={currentUser!} />}
        </Group>
        <MultiSelect
          clearable
          value={permissions}
          data={allPermissions.map((perm) => ({ value: perm, label: perm }))}
          label="Permissions for the user"
          onChange={setPermissions}
        ></MultiSelect>
        <Group align="right" mt="md">
          <Button type="submit" onClick={() => createAccess()}>
            Set access
          </Button>
        </Group>
      </Modal>
    </Stack>
  );
};

const SelectItem: React.FC<{ user: UserDto }> = ({ user }) => {
  console.log(user);
  return (
    <Group wrap="nowrap">
      <div>
        <Text size="sm">{user.username}</Text>
        <Text size="xs" opacity={0.65}>
          {user.email}
        </Text>
      </div>
    </Group>
  );
};

export default AccessControl;

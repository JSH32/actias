import { ProjectDto } from '@/client';
import { withAuthentication } from '@/helpers/authenticated';
import { Json } from '@/components/Json';
import { useStore } from '@/helpers/state';
import React, { useCallback, useEffect, useState } from 'react';
import api, { errorForm, showError } from '@/helpers/api';
import {
  Anchor,
  Button,
  Card,
  Grid,
  Group,
  Modal,
  Stack,
  Text,
  TextInput,
  Title,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { useForm } from '@mantine/form';
import { notifications } from '@mantine/notifications';
import { IconTrash } from '@tabler/icons-react';
import Link from 'next/link';

const User = () => {
  const store = useStore();

  const [projects, setProjects] = useState<ProjectDto[] | null>(null);

  const loadProjects = useCallback(() => {
    api.project.listProjects(0).then((projects) => {
      setProjects((projects as any).items);
    });
  }, []);

  // First load.
  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  const [createOpened, { open, close }] = useDisclosure(false);

  const createProjectForm = useForm({
    initialValues: {
      name: '',
    },
  });

  const createProject = useCallback(
    (values: any) => {
      api.project
        .createProject(values)
        .then((res) => {
          notifications.show({
            title: 'Project created!',
            message: `New project named ${res.name} was created.`,
          });

          projects?.push(res);
          close();
        })
        .catch((err) => errorForm(err.body, createProjectForm));

      createProjectForm.reset();
    },
    [projects, createProjectForm, close],
  );

  const deleteProject = useCallback(
    (project: ProjectDto) => {
      api.project
        .delete(project.id)
        .then(({ message }) => {
          notifications.show({
            title: 'Project deleted!',
            message,
          });

          loadProjects();
        })
        .catch((err) => showError(err));
    },
    [loadProjects],
  );

  return (
    <>
      <Json value={store?.userData} />
      <Stack>
        <Title>Projects</Title>
        <Button w={150} onClick={open}>
          Create Project
        </Button>

        <Modal opened={createOpened} onClose={close} title="Create Project">
          <form onSubmit={createProjectForm.onSubmit(createProject)}>
            <TextInput
              withAsterisk
              label="Name"
              placeholder="Project Name"
              {...createProjectForm.getInputProps('name')}
            />
            <Group position="right" mt="md">
              <Button type="submit">Submit</Button>
            </Group>
          </form>
        </Modal>

        <Grid gutter="xs">
          {projects?.map((project) => (
            <Grid.Col key={project.id} span="content">
              <ProjectCard
                key={project.id}
                project={project}
                onDelete={deleteProject}
              />
            </Grid.Col>
          ))}
        </Grid>
      </Stack>
    </>
  );
};

const ProjectCard: React.FC<{
  project: ProjectDto;
  onDelete: (project: ProjectDto) => void;
}> = ({ project, onDelete }) => {
  return (
    <Card shadow="sm" padding="lg" radius="md" withBorder w={400}>
      <Group position="apart" mt="md" mb="xs">
        <Title order={3}>{project.name}</Title>
        <Anchor component="button" onClick={() => onDelete(project)}>
          <IconTrash />
        </Anchor>
      </Group>

      <Text mt="xs" color="dimmed" size="sm">
        {project.id}
      </Text>

      <Link href={`/project/${project.id}`}>
        <Button variant="light" fullWidth mt="md" radius="md">
          Open
        </Button>
      </Link>
    </Card>
  );
};

export default withAuthentication(User);

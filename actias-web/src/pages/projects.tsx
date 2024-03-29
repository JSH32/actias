import { ProjectDto } from '@/client';
import { withAuthentication } from '@/helpers/authenticated';
import React, { useCallback, useEffect, useState } from 'react';
import api, { errorForm, showError } from '@/helpers/api';
import {
  Anchor,
  Button,
  Card,
  CloseButton,
  Divider,
  Grid,
  Group,
  Loader,
  Modal,
  Stack,
  Text,
  TextInput,
  Title,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { useForm } from '@mantine/form';
import { notifications } from '@mantine/notifications';
import Link from 'next/link';

const Projects = () => {
  const [projects, setProjects] = useState<ProjectDto[] | null>(null);

  const loadProjects = useCallback(() => {
    api.project.listProjects(1).then((projects) => {
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
        .catch((err) => errorForm(err, createProjectForm));

      createProjectForm.reset();
    },
    [projects, createProjectForm, close],
  );

  const deleteProject = useCallback(
    (project: ProjectDto) => {
      api.project
        .deleteProject(project.id)
        .then(({ message }) => {
          notifications.show({
            title: 'Project deleted!',
            message,
          });

          loadProjects();
        })
        .catch(showError);
    },
    [loadProjects],
  );

  return (
    <>
      <Stack>
        <Title>Projects</Title>
        <Divider />
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
            <Group align="right" mt="md">
              <Button type="submit">Submit</Button>
            </Group>
          </form>
        </Modal>

        {!projects && <Loader />}
        <Grid gutter="xs">
          {projects?.map((project) => (
            <Grid.Col key={project.id} span={{ md: 6, lg: 3 }}>
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
    <Card shadow="sm" padding="lg" radius="md" withBorder>
      <Group justify="space-between" mt="md" mb="xs">
        <Title order={3}>{project.name}</Title>
        <Anchor component="button" onClick={() => onDelete(project)}>
          <CloseButton aria-label="Delete project" />
        </Anchor>
      </Group>

      <Text mt="xs" color="dimmed" size="sm">
        {project.id}
      </Text>

      <Link href={`/project/${project.id}`}>
        <Button fullWidth mt="md" radius="md">
          Open
        </Button>
      </Link>
    </Card>
  );
};

export default withAuthentication(Projects);

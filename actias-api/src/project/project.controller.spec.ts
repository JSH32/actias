import { of } from 'rxjs';
import { ProjectController } from './project.controller';
import { ProjectPolicyDto, ProjectPolicyViewDto } from './dto/policy.dto';

const PROJECT = {
  id: 'project-1',
  name: 'p',
  owner: { id: 'user-1' },
  createdAt: new Date(0),
  updatedAt: new Date(0),
} as any;

/** A controller over recording mocks; only the policy paths matter. */
function controller() {
  const getProjectPolicy = jest.fn(() =>
    of({
      projectId: 'project-1',
      requestsPerSec: 50,
      workUnitsPerSec: 1000000,
      egressAllow: ['api.example.com'],
      egressDeny: [],
      region: 'eu-west',
      moving: false,
    }),
  );
  const setProjectPolicy = jest.fn((policy) =>
    of({ ...policy, region: 'eu-west', moving: false }),
  );
  const setProjectRegion = jest.fn((request) =>
    of({ projectId: request.projectId, region: request.region }),
  );
  const client = {
    getService: () => ({
      getProjectPolicy,
      setProjectPolicy,
      setProjectRegion,
    }),
  } as any;
  const projectService = {
    createProject: jest.fn(async () => PROJECT),
  } as any;
  const config = { get: () => 'local' } as any;
  const instance = new ProjectController(projectService, client, config);
  instance.onModuleInit();
  return { instance, getProjectPolicy, setProjectPolicy, setProjectRegion };
}

describe('the project policy', () => {
  it('reads what the script service holds', async () => {
    const { instance, getProjectPolicy } = controller();
    const policy = await instance.getPolicy(PROJECT);
    expect(getProjectPolicy).toHaveBeenCalledWith({ projectId: 'project-1' });
    expect(policy).toEqual(
      Object.assign(new ProjectPolicyViewDto(), {
        requestsPerSec: 50,
        workUnitsPerSec: 1000000,
        egressAllow: ['api.example.com'],
        egressDeny: [],
        region: 'eu-west',
        moving: false,
      }),
    );
  });

  it('records the home at creation: the ingress region, else its own', async () => {
    const { instance, setProjectRegion } = controller();
    await instance.createProject(
      {} as any,
      { name: 'p' } as any,
      {
        headers: { 'x-actias-region': 'ap-south' },
      } as any,
    );
    expect(setProjectRegion).toHaveBeenLastCalledWith({
      projectId: 'project-1',
      region: 'ap-south',
    });
    await instance.createProject(
      {} as any,
      { name: 'p' } as any,
      {
        headers: { 'x-actias-region': 'NOT A REGION' },
      } as any,
    );
    expect(setProjectRegion).toHaveBeenLastCalledWith({
      projectId: 'project-1',
      region: 'local',
    });
    await instance.createProject(
      {} as any,
      { name: 'p' } as any,
      {
        headers: {},
      } as any,
    );
    expect(setProjectRegion).toHaveBeenLastCalledWith({
      projectId: 'project-1',
      region: 'local',
    });
  });

  it('writes every field under the project id', async () => {
    const { instance, setProjectPolicy } = controller();
    const wanted = Object.assign(new ProjectPolicyDto(), {
      requestsPerSec: 10,
      workUnitsPerSec: 0,
      egressAllow: [],
      egressDeny: ['.internal.example'],
    });
    const stored = await instance.setPolicy(PROJECT, wanted);
    expect(setProjectPolicy).toHaveBeenCalledWith({
      projectId: 'project-1',
      requestsPerSec: 10,
      workUnitsPerSec: 0,
      egressAllow: [],
      egressDeny: ['.internal.example'],
    });
    expect(stored.egressDeny).toEqual(['.internal.example']);
    expect(stored.region).toEqual('eu-west');
  });
});

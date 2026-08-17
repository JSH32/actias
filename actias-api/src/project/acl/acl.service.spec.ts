import { AclService } from './acl.service';
import { AccessFields } from './accessFields';

describe('AclService', () => {
  it('grants a project owner every permission', async () => {
    // Dormant for http callers because AclGuard short-circuits owners, but
    // any direct caller (the live gateway) lands on this branch.
    const service = new AclService({} as any);
    const user = { id: 'user-1' } as any;
    const project = { owner: user } as any;

    const access = await service.getProjectAccess(user, project);

    expect(access.test(AccessFields.FULL)).toBe(true);
    expect(access.test(AccessFields.SCRIPT_RESOURCE)).toBe(true);
  });
});

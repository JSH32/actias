import { UnauthorizedException } from '@nestjs/common';
import { BitField } from 'easy-bits';
import { createHash } from 'crypto';

import { AuthGuard } from 'src/auth/auth.guard';
import { AuthService } from 'src/auth/auth.service';
import { AclGuard } from './acl/acl.guard';
import { AclService } from './acl/acl.service';
import { AccessFields } from './acl/accessFields';

// Real uuids: the acl guard runs the project param through requireUuid,
// so a readable placeholder would fail as a malformed id and never
// reach the authorization these tests are about.
const PROJECT = { id: '11111111-1111-4111-8111-111111111111' } as any;
const OTHER_PROJECT_ID = '22222222-2222-4222-8222-222222222222';

/** A stored token row carrying the given access bits. */
function tokenWith(bits: AccessFields) {
  return {
    id: 'token-1',
    project: PROJECT,
    permissionBitfield: new BitField().on(bits).serialize().toString(),
  } as any;
}

/** An execution context whose handler carries the given acl metadata. */
function contextFor(request: any) {
  return {
    switchToHttp: () => ({ getRequest: () => request }),
    getHandler: () => 'handler',
    getClass: () => class {},
  } as any;
}

describe('service token authentication', () => {
  it('resolves a stored token by hash and stamps its use', async () => {
    const token = 'actias_deadbeef';
    const row = tokenWith(AccessFields.SCRIPT_RESOURCE);
    const em = {
      findOne: jest.fn(async (_entity: unknown, where: any) =>
        where.tokenHash === createHash('sha256').update(token).digest('hex')
          ? row
          : null,
      ),
      flush: jest.fn(async () => undefined),
    };

    const subject = new AuthService({} as any, {} as any, em as any);

    const found = await subject.getServiceToken(token);

    expect(found).toBe(row);
    expect(found.lastUsed).toBeInstanceOf(Date);
    expect(em.flush).toHaveBeenCalled();
  });

  it('refuses a revoked token, whose hash no longer exists', async () => {
    // Revocation is deletion, so the lookup simply finds nothing.
    const em = {
      findOne: jest.fn(async () => null),
      flush: jest.fn(),
    };
    const subject = new AuthService({} as any, {} as any, em as any);

    await expect(subject.getServiceToken('actias_revoked')).rejects.toThrow(
      UnauthorizedException,
    );
  });

  it('authenticates a bearer service token onto the request', async () => {
    const row = tokenWith(AccessFields.SCRIPT_RESOURCE);
    const guard = new AuthGuard(
      { getServiceToken: jest.fn(async () => row) } as any,
      { get: jest.fn(() => undefined) } as any,
    );

    const request: any = {
      headers: { authorization: 'Bearer actias_deadbeef' },
    };

    await expect(guard.canActivate(contextFor(request))).resolves.toBe(true);
    expect(request.serviceToken).toBe(row);
    expect(request.user).toBeUndefined();
  });
});

describe('service token authorization', () => {
  function aclGuard(aclMeta: any) {
    return new AclGuard(
      new AclService({} as any),
      { findOneOrFail: jest.fn(async () => PROJECT) } as any,
      {
        get: jest.fn((key: string) => (key === 'acl' ? aclMeta : undefined)),
      } as any,
      {} as any,
    );
  }

  it('lets a token holding script access deploy in its project', async () => {
    const guard = aclGuard({ bitfield: AccessFields.SCRIPT_WRITE });
    const request: any = {
      serviceToken: tokenWith(AccessFields.SCRIPT_RESOURCE),
      params: { project: PROJECT.id },
    };

    await expect(guard.canActivate(contextFor(request))).resolves.toBe(true);
  });

  it('refuses a token outside its granted access', async () => {
    const guard = aclGuard({ bitfield: AccessFields.PERMISSIONS_WRITE });
    const request: any = {
      serviceToken: tokenWith(AccessFields.SCRIPT_RESOURCE),
      params: { project: PROJECT.id },
    };

    await expect(guard.canActivate(contextFor(request))).rejects.toThrow();
  });

  it('refuses a token inside a project it does not belong to', async () => {
    const token = tokenWith(AccessFields.FULL);
    token.project = { id: OTHER_PROJECT_ID };

    const guard = aclGuard({ bitfield: AccessFields.SCRIPT_READ });
    const request: any = {
      serviceToken: token,
      params: { project: PROJECT.id },
    };

    await expect(guard.canActivate(contextFor(request))).rejects.toThrow();
  });
});

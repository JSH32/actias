import { EntityManager } from '@mikro-orm/core';
import { BadRequestException, Injectable } from '@nestjs/common';
import { Projects } from 'src/entities/Projects';
import { Users } from 'src/entities/Users';
import { ProjectService } from 'src/project/project.service';
import { PaginatedResponseDto } from 'src/shared/dto/paginated';
import { UserDto } from 'src/users/dto/user.dto';
import { AdminProjectDto } from './dto/manage.dto';

/**
 * The instance admin's reach: every user and every project, regardless
 * of membership. Deletions ride the same service paths the owners use,
 * so an admin delete cleans up exactly what an owner delete would.
 */
@Injectable()
export class ManageService {
  constructor(
    private readonly em: EntityManager,
    private readonly projectService: ProjectService,
  ) {}

  async listUsers(
    page: number,
    pageSize: number,
    search?: string,
  ): Promise<PaginatedResponseDto<UserDto>> {
    const where = search
      ? {
          $or: [
            { username: { $ilike: `%${search}%` } },
            { email: { $ilike: `%${search}%` } },
          ],
        }
      : {};
    const [users, count] = await this.em.findAndCount(Users, where, {
      limit: pageSize,
      offset: (page - 1) * pageSize,
      orderBy: { createdAt: 'DESC' },
    });

    return PaginatedResponseDto.fromArray(
      page,
      Math.ceil(count / pageSize),
      users.map((user) => new UserDto(user)),
    );
  }

  async setAdmin(actor: Users, id: string, admin: boolean): Promise<UserDto> {
    if (actor.id === id) {
      throw new BadRequestException(
        'Your own admin flag is another admin’s to change.',
      );
    }
    const user = await this.em.findOneOrFail(Users, { id });
    user.admin = admin;
    await this.em.persistAndFlush(user);
    return new UserDto(user);
  }

  async deleteUser(actor: Users, id: string): Promise<void> {
    if (actor.id === id) {
      throw new BadRequestException('You cannot delete your own account.');
    }
    const user = await this.em.findOneOrFail(
      Users,
      { id },
      { populate: ['ownedProjects'] },
    );
    // Owned projects go through the same teardown an owner's delete
    // uses; only then does the user row itself go.
    for (const project of user.ownedProjects) {
      await this.projectService.deleteProject(project);
    }
    await this.em.removeAndFlush(user);
  }

  async listProjects(
    page: number,
    pageSize: number,
    search?: string,
  ): Promise<PaginatedResponseDto<AdminProjectDto>> {
    const where = search ? { name: { $ilike: `%${search}%` } } : {};
    const [projects, count] = await this.em.findAndCount(Projects, where, {
      limit: pageSize,
      offset: (page - 1) * pageSize,
      orderBy: { createdAt: 'DESC' },
      populate: ['owner'],
    });

    return PaginatedResponseDto.fromArray(
      page,
      Math.ceil(count / pageSize),
      projects.map((project) => new AdminProjectDto(project)),
    );
  }

  async deleteProject(id: string): Promise<Projects> {
    const project = await this.em.findOneOrFail(Projects, { id });
    await this.projectService.deleteProject(project);
    return project;
  }
}

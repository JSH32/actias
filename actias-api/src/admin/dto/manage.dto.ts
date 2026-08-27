import { IsBoolean } from 'class-validator';
import { Projects } from 'src/entities/Projects';

/** A project as the instance admin sees it: any project, with its owner. */
export class AdminProjectDto {
  id!: string;

  name!: string;

  /** The owning user's name; ownership means full access. */
  ownerUsername!: string;

  createdAt!: Date;

  constructor(project: Projects) {
    this.id = project.id;
    this.name = project.name;
    this.ownerUsername = project.owner?.username ?? '';
    this.createdAt = project.createdAt;
  }
}

/** Grants or revokes the instance admin flag. */
export class SetAdminDto {
  @IsBoolean()
  admin!: boolean;
}

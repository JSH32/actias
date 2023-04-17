import { Length } from 'class-validator';

export class CreateProjectDto {
  /**
   * Name of project.
   */
  @Length(6, 36)
  name!: string;
}

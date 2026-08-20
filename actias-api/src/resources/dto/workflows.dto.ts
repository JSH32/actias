import { ApiProperty } from '@nestjs/swagger';
import { IsOptional, IsString } from 'class-validator';

/** One workflow definition a live contract declares. */
export class WorkflowDefinitionDto {
  @ApiProperty()
  name: string;

  @ApiProperty({ description: 'Public identifier of the declaring script.' })
  declaredBy: string;

  @ApiProperty({
    description:
      'Step name literals found at publish: a superset of what may run, rendered as the hollow skeleton.',
    type: [String],
  })
  stepNames: string[];
}

/** One run row in the CI-runs table. */
export class WorkflowRunDto {
  @ApiProperty({ description: 'The caller-supplied run id.' })
  id: string;

  @ApiProperty()
  definition: string;

  @ApiProperty({
    description:
      'completed, cancelled, sleeping, awaiting, running or unstarted.',
  })
  status: string;

  @ApiProperty({
    description: 'The status detail: due times, awaited signal, reason.',
    type: 'object',
    additionalProperties: true,
  })
  detail: Record<string, unknown>;

  @ApiProperty({ description: 'Journal rows so far.' })
  entries: number;

  @ApiProperty({ description: 'The step or gate the run is at.' })
  atStep: string;

  @ApiProperty({
    description: 'The STARTED input, verbatim.',
    type: 'object',
    additionalProperties: true,
    nullable: true,
  })
  input: unknown;

  @ApiProperty({ required: false, nullable: true })
  startedAt?: number;

  @ApiProperty({ required: false, nullable: true })
  updatedAt?: number;
}

/** One journal row, as the forensics tab shows it. */
export class WorkflowJournalRowDto {
  @ApiProperty()
  seq: number;

  @ApiProperty()
  at: number;

  @ApiProperty({
    description:
      'STARTED, INTENT, RESULT, TIMER, SIGNAL, CHILD, CANCEL, COMPLETED or AMBIENT.',
  })
  kind: string;

  @ApiProperty({ type: 'object', additionalProperties: true })
  data: Record<string, unknown>;

  @ApiProperty()
  format: number;
}

/** One run, whole: what the CI view folds. */
export class WorkflowRunDetailDto {
  @ApiProperty()
  id: string;

  @ApiProperty()
  definition: string;

  @ApiProperty()
  status: string;

  @ApiProperty({ type: 'object', additionalProperties: true })
  detail: Record<string, unknown>;

  @ApiProperty({ type: [WorkflowJournalRowDto] })
  journal: WorkflowJournalRowDto[];
}

export class RunStartDto {
  @ApiProperty({ required: false, type: 'object', additionalProperties: true })
  @IsOptional()
  payload?: unknown;

  @ApiProperty({
    required: false,
    description:
      'The run id; the idempotency key. Omitted, the console mints one.',
  })
  @IsOptional()
  @IsString()
  id?: string;
}

export class RunSignalDto {
  @ApiProperty()
  @IsString()
  name: string;

  @ApiProperty({ required: false, type: 'object', additionalProperties: true })
  @IsOptional()
  payload?: unknown;
}

export class RunCancelDto {
  @ApiProperty({ required: false })
  @IsOptional()
  @IsString()
  reason?: string;
}

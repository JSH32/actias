import { Body, Controller, Get, Param, Post } from '@nestjs/common';
import { ApiParam, ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';
import { AclByProject } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { ResourcesService } from './resources.service';
import {
  RunSignalDto,
  RunCancelDto,
  WorkflowDefinitionDto,
  WorkflowRunDetailDto,
  WorkflowRunDto,
} from './dto/workflows.dto';

/** The platform class a workflow run rides on. */
const WORKFLOW_CLASS = '__workflow';

/**
 * A project's durable workflows: definitions come from contracts, runs
 * from the instance directory, and everything a run view shows is a
 * fold over its replay journal, read from the freshest copy without
 * waking the vm. One family of the backplane (`/project/:id/workflows`).
 */
@ApiTags('workflows')
@Controller('project/:project/workflows')
export class WorkflowsController {
  constructor(private readonly resources: ResourcesService) {}

  /** Every workflow definition a live contract declares, with the
   * declared-possible step names the skeleton renders. */
  @Get()
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listDefinitions(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<WorkflowDefinitionDto[]> {
    const page = await lastValueFrom(
      this.resources.scripts
        .listScripts({ projectId: project.id, pageSize: 500, page: 1 })
        .pipe(toHttpException()),
    );
    const definitions: WorkflowDefinitionDto[] = [];
    for (const script of page.scripts || []) {
      if (!script.currentRevisionId) continue;
      const revision = await lastValueFrom(
        this.resources.scripts
          .getRevision({
            id: script.currentRevisionId,
            withBundle: false,
            manifestOnly: false,
          })
          .pipe(toHttpException()),
      );
      const capabilities = revision.scriptConfig?.capabilities;
      for (const name of capabilities?.workflows ?? []) {
        definitions.push({
          name,
          declaredBy: script.publicIdentifier,
          stepNames: capabilities?.workflowSteps ?? [],
        });
      }
    }
    return definitions.sort((a, b) => a.name.localeCompare(b.name));
  }

  /** The definition's runs, newest first, each with its journal-derived
   * status; the directory names them, the files answer for them. */
  @Get(':definition/runs')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listRuns(
    @EntityParam('project', Projects) project: Projects,
    @Param('definition') definition: string,
  ): Promise<WorkflowRunDto[]> {
    const directory = await lastValueFrom(
      this.resources.registry
        .listInstances({
          projectIds: [project.id],
          class: WORKFLOW_CLASS,
          namePrefix: `${definition}/`,
          pageSize: 100,
        })
        .pipe(toHttpException()),
    );

    const runs = await Promise.all(
      (directory.instances || []).map(async (instance) => {
        const head = (await this.resources.workerRead(
          project,
          WORKFLOW_CLASS,
          instance.name,
        )) as {
          status?: Record<string, unknown>;
          entries?: number;
          started_at?: number;
          updated_at?: number;
        } | null;
        return {
          id: instance.name.slice(definition.length + 1),
          definition,
          status: String(head?.status?.status ?? 'unstarted'),
          detail: head?.status ?? {},
          entries: Number(head?.entries ?? 0),
          startedAt: head?.started_at ?? undefined,
          updatedAt: head?.updated_at ?? undefined,
        };
      }),
    );
    return runs.sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0));
  }

  /** One run, whole: status plus the journal the CI view folds. */
  @Get(':definition/runs/:id')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async runDetail(
    @EntityParam('project', Projects) project: Projects,
    @Param('definition') definition: string,
    @Param('id') id: string,
  ): Promise<WorkflowRunDetailDto> {
    const name = `${definition}/${id}`;
    const journal = await this.resources.readJournal(
      project,
      WORKFLOW_CLASS,
      name,
      0,
    );
    const rows = Array.isArray(journal) ? journal : [];
    const head = (await this.resources.workerRead(
      project,
      WORKFLOW_CLASS,
      name,
    )) as { status?: Record<string, unknown> } | null;
    return {
      id,
      definition,
      status: String(head?.status?.status ?? 'unstarted'),
      detail: head?.status ?? {},
      journal: rows as WorkflowRunDetailDto['journal'],
    };
  }

  /** Delivers a named signal into the run; a parked await resumes. */
  @Post(':definition/runs/:id/signal')
  @AclByProject(AccessFields.SCRIPT_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async signal(
    @EntityParam('project', Projects) project: Projects,
    @Param('definition') definition: string,
    @Param('id') id: string,
    @Body() body: RunSignalDto,
  ): Promise<Record<string, unknown>> {
    const outcome = await this.resources.dispatchObject(
      project,
      WORKFLOW_CLASS,
      `${definition}/${id}`,
      'signal',
      [body.name, body.payload ?? null],
    );
    return (outcome ?? {}) as Record<string, unknown>;
  }

  /** Cancels the run; children and late signals stay refused. */
  @Post(':definition/runs/:id/cancel')
  @AclByProject(AccessFields.SCRIPT_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async cancel(
    @EntityParam('project', Projects) project: Projects,
    @Param('definition') definition: string,
    @Param('id') id: string,
    @Body() body: RunCancelDto,
  ): Promise<Record<string, unknown>> {
    const outcome = await this.resources.dispatchObject(
      project,
      WORKFLOW_CLASS,
      `${definition}/${id}`,
      'cancel',
      [body.reason ?? 'cancelled from the console'],
    );
    return (outcome ?? {}) as Record<string, unknown>;
  }
}

import { Controller, Get, Param, Post, Query, UseGuards } from '@nestjs/common';
import { ApiParam, ApiQuery, ApiTags } from '@nestjs/swagger';
import { AuthGuard } from 'src/auth/auth.guard';
import { AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { CLASSES, ResourcesService } from './resources.service';
import {
  QueueEventDto,
  QueueMessageDto,
  QueueStatsDto,
  ResourceInstanceDto,
  RetriedDto,
} from './dto/resources.dto';

/**
 * A project's queues: declared by contracts or remembered by the
 * directory, inspected and controlled through the worker data plane.
 * One family of the backplane surface (`/project/:id/queues`).
 */
@UseGuards(AuthGuard, AclGuard)
@ApiTags('queues')
@Controller('project/:project/queues')
export class QueuesController {
  constructor(private readonly resources: ResourcesService) {}

  @Get()
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listQueues(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ResourceInstanceDto[]> {
    return this.resources.listResources(project, 'queues');
  }

  @Get(':name/stats')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async queueStats(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<QueueStatsDto> {
    const stats = (await this.resources.workerRead(
      project,
      CLASSES.queues,
      name,
    )) as {
      depth?: number;
      in_flight?: number;
      oldest_pending?: number;
      dead_letters?: number;
    } | null;
    return {
      depth: stats?.depth ?? 0,
      inFlight: stats?.in_flight ?? 0,
      oldestPending: stats?.oldest_pending ?? undefined,
      deadLetters: stats?.dead_letters ?? 0,
    };
  }

  /** Live and dead message rows, newest first; delivered messages are in
   * the journal. */
  @Get(':name/messages')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async queueMessages(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<QueueMessageDto[]> {
    const rows = (await this.resources.workerRead(
      project,
      CLASSES.queues,
      name,
      { messages: true },
    )) as Record<string, unknown>[] | null;
    return (Array.isArray(rows) ? rows : []).map((row) => ({
      id: Number(row.id),
      state: String(row.state ?? ''),
      attempts: Number(row.attempts ?? 0),
      preview: String(row.preview ?? ''),
      size: Number(row.size ?? 0),
      enqueuedMs: Number(row.enqueued_ms ?? 0),
      nextMs: row.next_ms == null ? undefined : Number(row.next_ms),
      diedMs: row.died_ms == null ? undefined : Number(row.died_ms),
    }));
  }

  /** The queue's journal after `since`: enqueued, delivered, retried and
   * dead-lettered, oldest first. */
  @Get(':name/events')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  @ApiQuery({ name: 'since', required: false, type: Number })
  async queueEvents(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Query('since') since?: string,
  ): Promise<QueueEventDto[]> {
    const events = await this.resources.readJournal(
      project,
      CLASSES.queues,
      name,
      Number(since ?? 0),
    );
    return Array.isArray(events) ? (events as QueueEventDto[]) : [];
  }

  /** Requeues every dead letter; they start their attempts over. */
  @Post(':name/retry-dead')
  @AclByProject(AccessFields.SCRIPT_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async retryDead(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<RetriedDto> {
    const count = await this.resources.dispatchObject(
      project,
      CLASSES.queues,
      name,
      'retry_dead',
      [],
    );
    return { requeued: Number(count ?? 0) };
  }

  /** Requeues one dead letter by id. */
  @Post(':name/messages/:id/retry')
  @AclByProject(AccessFields.SCRIPT_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async retryMessage(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Param('id') id: string,
  ): Promise<RetriedDto> {
    const count = await this.resources.dispatchObject(
      project,
      CLASSES.queues,
      name,
      'retry_message',
      [Number(id)],
    );
    return { requeued: Number(count ?? 0) };
  }

  /** Discards one message, live or dead. */
  @Post(':name/messages/:id/drop')
  @AclByProject(AccessFields.SCRIPT_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async dropMessage(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Param('id') id: string,
  ): Promise<void> {
    await this.resources.dispatchObject(
      project,
      CLASSES.queues,
      name,
      'drop_message',
      [Number(id)],
    );
  }
}

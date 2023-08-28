import { EventSubscriber, EntityManager, EventArgs } from '@mikro-orm/core';
import { Injectable, OnModuleInit, Inject, Logger } from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { catchError, lastValueFrom, of } from 'rxjs';
import { Projects } from 'src/entities/Projects';
import { kv_service } from 'src/protobufs/kv_service';
import { script_service } from 'src/protobufs/script_service';

@Injectable()
export class ProjectSubscriber
  implements EventSubscriber<Projects>, OnModuleInit
{
  private scriptService: script_service.ScriptService;
  private kvService: kv_service.KvService;

  private readonly logger = new Logger(ProjectSubscriber.name);

  constructor(
    @Inject('SCRIPT_SERVICE') private readonly scriptClient: ClientGrpc,
    @Inject('KV_SERVICE') private readonly kvClient: ClientGrpc,
    em: EntityManager,
  ) {
    em.getEventManager().registerSubscriber(this);
  }

  onModuleInit() {
    this.scriptService =
      this.scriptClient.getService<script_service.ScriptService>(
        'ScriptService',
      );

    this.kvService =
      this.kvClient.getService<kv_service.KvService>('KvService');
  }

  // We don't care if this fails, we can't do anything about it.
  public async afterDelete(args: EventArgs<Projects>) {
    await lastValueFrom(
      this.scriptService
        .deleteProject({
          projectId: args.entity.id,
        })
        .pipe(catchError(() => of(undefined))),
    );

    await lastValueFrom(
      this.kvService
        .deleteProject({ projectId: args.entity.id })
        .pipe(catchError(() => of(undefined))),
    );
  }
}

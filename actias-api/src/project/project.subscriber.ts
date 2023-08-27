import { EventSubscriber, EntityManager, EventArgs } from '@mikro-orm/core';
import { Injectable, OnModuleInit, Inject } from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { lastValueFrom } from 'rxjs';
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { script_service } from 'src/protobufs/script_service';

@Injectable()
export class ProjectSubscriber
  implements EventSubscriber<Projects>, OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(
    @Inject('SCRIPT_SERVICE') private readonly client: ClientGrpc,
    em: EntityManager,
  ) {
    em.getEventManager().registerSubscriber(this);
  }

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  public async afterDelete(args: EventArgs<Projects>) {
    lastValueFrom(
      await this.scriptService
        .deleteProject({
          projectId: args.entity.id,
        })
        .pipe(toHttpException()),
    );
  }
}

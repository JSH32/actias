import {
  EventSubscriber,
  EntityManager,
  EventArgs,
  Subscriber,
} from '@mikro-orm/core';
import { Injectable, OnModuleInit, Inject } from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { lastValueFrom } from 'rxjs';
import { Resources, ResourceType } from 'src/entities/Resources';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { script_service } from 'src/protobufs/script_service';

@Injectable()
export class ScriptSubscriber
  implements EventSubscriber<Resources>, OnModuleInit {
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

  public async afterDelete(args: EventArgs<Resources>) {
    if (args.entity.resourceType === ResourceType.SCRIPT) {
      lastValueFrom(
        await this.scriptService
          .deleteScript({
            scriptId: args.entity.serviceId,
          })
          .pipe(toHttpException()),
      );
    }
  }
}

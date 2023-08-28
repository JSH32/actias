import { NestFactory } from '@nestjs/core';
import { AppModule } from './app.module';
import { SwaggerModule, DocumentBuilder } from '@nestjs/swagger';
import { ConfigService } from '@nestjs/config';
import { BadRequestException, ValidationPipe } from '@nestjs/common';
import {
  FastifyAdapter,
  NestFastifyApplication,
} from '@nestjs/platform-fastify';

async function bootstrap() {
  const app = await NestFactory.create<NestFastifyApplication>(
    AppModule,
    new FastifyAdapter({ logger: true, bodyLimit: 209_715_200 }),
  );

  app.useGlobalPipes(
    new ValidationPipe({
      transform: true,
      exceptionFactory: (errors) => {
        const newErrors = {};

        for (const error of errors) {
          const key = error.property;

          // Remove the field name from the beginning,
          // capitalize first letter, add period at the end
          const value =
            Object.values(error.constraints)[0]
              .split(' ')
              .slice(1)
              .join(' ')
              .replace(/^\w/, (c) => c.toUpperCase()) + '.';

          newErrors[key] = value;
        }

        return new BadRequestException({
          statusCode: 400,
          message: 'Validation failed',
          errors: newErrors,
        });
      },
    }),
  );

  app.setGlobalPrefix('api');

  const config = new DocumentBuilder()
    .setTitle('Actias API')
    .setDescription('Public facing API for Actias workers.')
    .setVersion('1.0')
    .addTag('scripts', 'revisions')
    .addBearerAuth()
    .build();

  const document = SwaggerModule.createDocument(app, config, {
    operationIdFactory: (_, methodKey) => methodKey,
  });
  SwaggerModule.setup('/api/docs', app, document);

  app
    .getHttpAdapter()
    .get('/api/docs/openapi.json', (_, res) => res.send(document));

  const configService = app.get(ConfigService);
  await app.listen(configService.get<number>('port'));
}

bootstrap();

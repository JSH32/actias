import { Module } from '@nestjs/common';
import { ConfigModule } from '@nestjs/config';
import config from './config';
import { ScriptModule } from './scripts/scripts.module';

@Module({
  imports: [
    ConfigModule.forRoot({
      load: [config],
    }),
    ScriptModule,
  ],
  controllers: [],
  providers: [],
})
export class AppModule {}

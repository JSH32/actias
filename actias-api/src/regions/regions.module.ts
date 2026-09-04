import { Module, forwardRef } from '@nestjs/common';
import { AuthModule } from 'src/auth/auth.module';
import { ScriptModule } from 'src/scripts/scripts.module';
import { RegionsController } from './regions.controller';

@Module({
  controllers: [RegionsController],
  imports: [AuthModule, forwardRef(() => ScriptModule)],
})
export class RegionsModule {}

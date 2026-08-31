import { Module } from '@nestjs/common';
import { AuthModule } from 'src/auth/auth.module';
import { InterestController } from './interest.controller';
import { InterestService } from './interest.service';

@Module({
  imports: [AuthModule],
  controllers: [InterestController],
  providers: [InterestService],
})
export class InterestModule {}

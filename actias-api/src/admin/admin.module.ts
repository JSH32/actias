import { Module } from '@nestjs/common';
import { RegistrationController } from './registration.controller';
import { RegistrationService } from './registration.service';
import { AuthModule } from 'src/auth/auth.module';

@Module({
  imports: [AuthModule],
  exports: [RegistrationService],
  controllers: [RegistrationController],
  providers: [RegistrationService],
})
export class AdminModule {}

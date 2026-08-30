import { Module } from '@nestjs/common';
import { RegistrationController } from './registration.controller';
import { RegistrationService } from './registration.service';
import { BootstrapService } from './bootstrap.service';
import { MailService } from './mail.service';
import { ManageController } from './manage.controller';
import { ManageService } from './manage.service';
import { AuthModule } from 'src/auth/auth.module';
import { ProjectModule } from 'src/project/project.module';

@Module({
  imports: [AuthModule, ProjectModule],
  exports: [RegistrationService],
  controllers: [RegistrationController, ManageController],
  providers: [
    RegistrationService,
    BootstrapService,
    MailService,
    ManageService,
  ],
})
export class AdminModule {}

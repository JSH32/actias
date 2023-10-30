export class RegistrationCodeDto {
  id!: string;
  createdAt!: Date;
  updatedAt!: Date;
  uses!: number;

  constructor(registrationCode: RegistrationCodeDto) {
    Object.assign(this, registrationCode);
  }
}

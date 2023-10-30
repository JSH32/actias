export class RegistrationConfigDto {
  inviteOnly!: boolean;

  constructor(config: Required<RegistrationConfigDto>) {
    return Object.assign(this, config);
  }
}

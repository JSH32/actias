/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

export type RegistrationSettingsDto = {
    /**
     * Whether this instance requires a code to register.
     */
    inviteOnly: boolean;
    /**
     * Whether invites can be mailed; false means links are copied.
     */
    smtpEnabled: boolean;
};


/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { BaseHttpRequest } from './core/BaseHttpRequest';
import type { OpenAPIConfig } from './core/OpenAPI';
import { AxiosHttpRequest } from './core/AxiosHttpRequest';

import { AclService } from './services/AclService';
import { AuthService } from './services/AuthService';
import { ProjectService } from './services/ProjectService';
import { RevisionsService } from './services/RevisionsService';
import { ScriptsService } from './services/ScriptsService';
import { UsersService } from './services/UsersService';

type HttpRequestConstructor = new (config: OpenAPIConfig) => BaseHttpRequest;

export class ActiasClient {

    public readonly acl: AclService;
    public readonly auth: AuthService;
    public readonly project: ProjectService;
    public readonly revisions: RevisionsService;
    public readonly scripts: ScriptsService;
    public readonly users: UsersService;

    public readonly request: BaseHttpRequest;

    constructor(config?: Partial<OpenAPIConfig>, HttpRequest: HttpRequestConstructor = AxiosHttpRequest) {
        this.request = new HttpRequest({
            BASE: config?.BASE ?? '',
            VERSION: config?.VERSION ?? '1.0',
            WITH_CREDENTIALS: config?.WITH_CREDENTIALS ?? false,
            CREDENTIALS: config?.CREDENTIALS ?? 'include',
            TOKEN: config?.TOKEN,
            USERNAME: config?.USERNAME,
            PASSWORD: config?.PASSWORD,
            HEADERS: config?.HEADERS,
            ENCODE_PATH: config?.ENCODE_PATH,
        });

        this.acl = new AclService(this.request);
        this.auth = new AuthService(this.request);
        this.project = new ProjectService(this.request);
        this.revisions = new RevisionsService(this.request);
        this.scripts = new ScriptsService(this.request);
        this.users = new UsersService(this.request);
    }
}


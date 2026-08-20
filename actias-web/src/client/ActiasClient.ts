/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { BaseHttpRequest } from './core/BaseHttpRequest';
import type { OpenAPIConfig } from './core/OpenAPI';
import { AxiosHttpRequest } from './core/AxiosHttpRequest';

import { AclService } from './services/AclService';
import { AdminService } from './services/AdminService';
import { AuthService } from './services/AuthService';
import { KvService } from './services/KvService';
import { ProjectService } from './services/ProjectService';
import { ResourcesService } from './services/ResourcesService';
import { RevisionsService } from './services/RevisionsService';
import { ScriptsService } from './services/ScriptsService';
import { SecretsService } from './services/SecretsService';
import { TokensService } from './services/TokensService';
import { UsersService } from './services/UsersService';

type HttpRequestConstructor = new (config: OpenAPIConfig) => BaseHttpRequest;

export class ActiasClient {

    public readonly acl: AclService;
    public readonly admin: AdminService;
    public readonly auth: AuthService;
    public readonly kv: KvService;
    public readonly project: ProjectService;
    public readonly resources: ResourcesService;
    public readonly revisions: RevisionsService;
    public readonly scripts: ScriptsService;
    public readonly secrets: SecretsService;
    public readonly tokens: TokensService;
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
        this.admin = new AdminService(this.request);
        this.auth = new AuthService(this.request);
        this.kv = new KvService(this.request);
        this.project = new ProjectService(this.request);
        this.resources = new ResourcesService(this.request);
        this.revisions = new RevisionsService(this.request);
        this.scripts = new ScriptsService(this.request);
        this.secrets = new SecretsService(this.request);
        this.tokens = new TokensService(this.request);
        this.users = new UsersService(this.request);
    }
}


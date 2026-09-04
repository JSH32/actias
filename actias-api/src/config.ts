export default () => ({
  port: parseInt(process.env.PORT, 10),
  externalServices: {
    scriptServiceUri: process.env.SCRIPT_SERVICE_URL,
    placementServiceUri: process.env.PLACEMENT_SERVICE_URL,
    kvServiceUri: process.env.KV_SERVICE_URL,
    secretServiceUri: process.env.SECRET_SERVICE_URL,
  },
  databaseUrl: process.env.DATABASE_URL,
  // A read replica for the console's reads; the primary when unset.
  readDatabaseUrl: process.env.READ_DATABASE_URL || undefined,
  // The region this control plane runs in: the home of a project created
  // through it when the regional ingress sent no x-actias-region.
  region: process.env.REGION || 'local',
  jwtKey: process.env.JWT_KEY,
  webOrigin: process.env.WEB_ORIGIN,
  // Cluster-internal worker access for dashboard work: object dispatch
  // and typed reads over the WorkerData grpc service. Same secret the
  // workers use between themselves; development default matches theirs.
  worker: {
    grpcUrl: process.env.WORKER_GRPC_URL || 'localhost:3100',
    internalToken: process.env.INTERNAL_TOKEN || 'dev-internal-token',
  },
  inviteOnly: process.env.INVITE_ONLY === 'true',
  // First-run admin for self-hosted instances; all three or nothing.
  bootstrapAdmin: {
    username: process.env.ADMIN_USERNAME,
    email: process.env.ADMIN_EMAIL,
    password: process.env.ADMIN_PASSWORD,
  },
  // Outbound mail, optional: SMTP_HOST unset means invites are links
  // the admin copies instead of messages the instance sends.
  smtp: {
    host: process.env.SMTP_HOST,
    port: parseInt(process.env.SMTP_PORT || '587', 10),
    user: process.env.SMTP_USER,
    pass: process.env.SMTP_PASS,
    from: process.env.SMTP_FROM,
  },
});
